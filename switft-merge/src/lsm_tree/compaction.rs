// BASE BRANCH Issue #21
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;

use crate::memtable::mem::ConfigInfo;
#[derive(PartialEq, Eq, Hash)]
pub enum CompactionEvents {
    Init,               // for config changes
    CompactionFinished, // ssTableReader needs to know to update indexes : (input ss-tables,output-sstables)
}
#[derive(Debug)]
pub enum DataSectionErr {
    NotSorted(Vec<u8>, Vec<u8>), // prev key, cur key do not have a prev < cur relationship
}

#[derive(Debug)]
pub enum TableCorruption {
    InvalidCRC(String),               //crc that was there instead of the correct one
    InvalidFooter(String),            // footer places in wrong space?
    InvalidSparseIndex(),             // missing sparse index len or missing sparse index
    DataSectionError(DataSectionErr), // what ever other error will be this
}
#[derive(Debug)]
pub enum CompactionError {
    IoError(io::Error),
    InvalidTable(String, TableCorruption), //(file name,issue) -> should be able to continue but will tell caller
}
impl From<io::Error> for CompactionError {
    fn from(err: io::Error) -> Self {
        CompactionError::IoError(err)
    }
}

pub struct CompactionCoordinator {
    update_funcs: HashMap<CompactionEvents, Vec<Box<dyn FnMut() + Send + Sync>>>, // pass in functions to call once compaction even occures
    config: ComapctionCofig,
    compact_by: std::time::Instant,
}
impl CompactionCoordinator {
    pub fn new(
        config: &ConfigInfo,
        caller_events: Vec<(CompactionEvents, Box<dyn FnMut() + Send + Sync>)>,
    ) -> Self {
        let mut update_funcs: HashMap<CompactionEvents, Vec<Box<dyn FnMut() + Send + Sync>>> =
            HashMap::new();
        for (event, func) in caller_events.into_iter() {
            update_funcs
                .entry(event)
                .or_insert_with(Vec::new)
                .push(func);
        }
        // generate a seperate thread that will do the monitoring
        Self {
            update_funcs,
            config: ComapctionCofig::new(&config),
            compact_by: std::time::Instant::now()
                + std::time::Duration::from_secs(config.compaction_check_interval_seconds as u64),
        }
    }

    pub fn monitor(compaction_monitor: Arc<tokio::sync::RwLock<CompactionCoordinator>>) {
        tokio::spawn(async move {
            loop {
                {
                    let data_dir = std::path::Path::new("data");
                    if !data_dir.exists() {
                        fs::create_dir_all(data_dir).await.unwrap();
                    }

                    // Loop through data/l1 to data/l10
                    for i in 1..=10 {
                        let level_dir_name = format!("l{}", i);
                        let level_dir = data_dir.join(&level_dir_name);

                        if !level_dir.exists() {
                            fs::create_dir_all(&level_dir).await.unwrap();
                        }

                        let mut read_dir = fs::read_dir(&level_dir).await.unwrap();
                        let mut small_ss_table_count = 0;
                        let mut tables = vec![];
                        while let Some(entry) = read_dir.next_entry().await.unwrap() {
                            let file_name = entry.file_name();
                            if let Some(ext) = file_name.to_str().and_then(|s| s.split('.').last())
                            {
                                if ext == "bin" {
                                    small_ss_table_count += 1;
                                    tables.push(String::from(file_name.to_str().unwrap()));
                                }
                            }
                        }

                        let mut lock = compaction_monitor.write().await;
                        if std::time::Instant::now() >= lock.compact_by
                            || small_ss_table_count > (lock.config.target_chunks as usize).pow(2)
                        {
                            //let data_dir_path = format!("data/{}", level_dir_name);
                            let _ = lock.size_tier_compaction(&level_dir, i, tables).await;
                            lock.compact_by = std::time::Instant::now() // reset 
                                + std::time::Duration::from_secs(
                                    lock.config.compaction_check_interval_seconds as u64,
                                );
                        }
                        drop(lock); // Release lock before next iteration
                    }
                }
                let interval = {
                    let read_lock = compaction_monitor.read().await;
                    read_lock.config.compaction_check_interval_seconds
                };
                println!("waiting for {} seconds", (interval / 4));
                tokio::time::sleep(std::time::Duration::from_secs(std::cmp::max(
                    10, // wait atleast 10 seconds
                    (interval as u64) / 4,
                )))
                .await;
            }
        });
    }
    async fn size_tier_compaction(
        &mut self,
        dir: &PathBuf,
        dir_level: u8,
        table_names: Vec<String>,
    ) -> Result<(), CompactionError> {
        println!(
            "starting size tier compaction for directory: {:?} with {} files",
            dir,
            table_names.len()
        );

        let target_chunks = self.config.target_chunks as usize;
        let mut compaction_units = Vec::new();

        // Split into compaction units
        let mut i = 0;
        while i + target_chunks <= table_names.len() {
            // Create units with target_chunks files (e.g., 4 files)
            let unit_files = table_names[i..i + target_chunks].to_vec();
            compaction_units.push(CompactionUnit {
                target_files: unit_files,
                consumed_files: Vec::new(),
            });
            i += target_chunks;
        }

        // Handle remaining files - only if we have at least 2
        let remaining = table_names.len() - i;
        if remaining >= 2 {
            // if not this file , save for next compaction cyle
            let unit_files = table_names[i..].to_vec();
            compaction_units.push(CompactionUnit {
                target_files: unit_files,
                consumed_files: Vec::new(),
            });
        }

        println!("Created {:?} compaction units", compaction_units);

        // Compact each unit
        let mut compacted_tmp_files = Vec::new();
        for mut unit in compaction_units {
            match unit.compact().await {
                Ok(result) => compacted_tmp_files.push(result),
                Err(e) => eprintln!("Compaction unit failed: {:?}", e),
            }
        }
        // check for data/l_current+1 exist, if it does do nothing, otherwise create it

        let base = std::path::Path::new("data");
        let next_level = base.join(format!("l{}", dir_level + 1));
        if !next_level.exists() {
            tokio::fs::create_dir(&next_level).await?;
        }

        //turn each .tmp into a .bin and move them to the next directory up
        for compacted_file in compacted_tmp_files.iter().take(1) {
            let new_name = compacted_file.replace(".tmp", ".bin");

            // Source: current directory + filename
            let source_path = dir.join(compacted_file); // data/l1/test_drive.tmp

            // Destination: next level + new name
            let dest_path = next_level.join(&new_name); // data/l2/test_drive.bin

            println!(
                "Moving from {} to {}",
                source_path.display(),
                dest_path.display()
            );
            tokio::fs::rename(&source_path, &dest_path).await?;
            // old_name -> new_name
            // current_directory -> cur+1_directory
        }
        // build summary table (min,max,level wide bloom filter)
        self.build_level_summary().await?;
        if let Some(compact_finish_funcs) = self
            .update_funcs
            .get_mut(&CompactionEvents::CompactionFinished)
        {
            for func in compact_finish_funcs.iter_mut() {
                func();
            }
        }

        Ok(())
    }
    async fn build_level_summary(&mut self) -> Result<(), CompactionError> {
        Ok(())
    }
}
#[derive(Debug)]
struct CompactionUnit {
    target_files: Vec<String>,
    consumed_files: Vec<String>,
}
impl CompactionUnit {
    // output the final compacted file (must still be .tmp) include the directory
    async fn compact(&mut self) -> Result<String, CompactionError> {
        Ok(String::from("test_drive.tmp"))
    }
}
struct ComapctionCofig {
    compaction_check_interval_seconds: u16,
    max_compaction_threads: u8,
    target_chunks: u8,
    local_disk: bool, // ignore this for now
}
impl ComapctionCofig {
    fn new(config: &ConfigInfo) -> Self {
        Self {
            compaction_check_interval_seconds: config.compaction_check_interval_seconds,
            max_compaction_threads: config.max_compaction_threads,
            target_chunks: config.target_chunks,
            local_disk: config.local_disk,
        }
    }
}
