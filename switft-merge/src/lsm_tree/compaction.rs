// BASE BRANCH Issue #21
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use tokio::fs;
//use tokio::sync::RwLock;

use crate::memtable::mem::ConfigInfo;
#[derive(PartialEq, Eq, Hash)]
pub enum CompactionEvents {
    Init, // for config changes
    CompactionStarted,
    CompactionFinished(u16, u16), // ssTableReader needs to know to update indexes : (input ss-tables,output-sstables)
}
pub enum DataSectionErr {
    NotSorted(Vec<u8>, Vec<u8>), // prev key, cur key do not have a prev < cur relationship
}
pub enum TableCorruption {
    InvalidCRC(String),               //crc that was there instead of the correct one
    InvalidFooter(String),            // footer places in wrong space?
    InvalidSparseIndex(),             // missing sparse index len or missing sparse index
    DataSectionError(DataSectionErr), // what ever other error will be this
}
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
                    let l1_dir = data_dir.join("l1");
                    if !l1_dir.exists() {
                        fs::create_dir_all(&l1_dir).await.unwrap();
                    }
                    let mut read_dir = fs::read_dir(l1_dir).await.unwrap();
                    let mut small_ss_table_count = 0;
                    while let Some(_entry) = read_dir.next_entry().await.unwrap() {
                        small_ss_table_count += 1;
                    }
                    let mut lock = compaction_monitor.write().await;
                    if std::time::Instant::now() >= lock.compact_by
                        || small_ss_table_count > (lock.config.target_chunks as usize).pow(2)
                    {
                        let _ = lock.size_tier_compaction().await;
                        lock.compact_by = std::time::Instant::now() // reset 
                            + std::time::Duration::from_secs(
                                lock.config.compaction_check_interval_seconds as u64,
                            );
                    }
                }
                let interval = {
                    let read_lock = compaction_monitor.read().await;
                    read_lock.config.compaction_check_interval_seconds
                };
                println!("waiting for {} seconds", (interval / 4));
                tokio::time::sleep(std::time::Duration::from_secs(std::cmp::max(
                    10,
                    (interval as u64) / 4,
                )))
                .await;
            }
        });

        println!("leaving Compaction.monitor");
        // look over the /data directory and when certain propertys are met I.E config-file params are met then call size-tier-compaction
        // even if certain properties are met on the config file level EX: now() > compactionCheckIntervalSeconds
        // if there arent enough files do not start compaction. we want to run compaction as less frequently as we can because its alot of IO
        // also for every level past level 1 theres should be a summary.index file that includes , level wide statistics (min,max key) , level wide bloom filter
    }
    async fn size_tier_compaction(&mut self) -> Result<(), CompactionError> {
        println!("starting size tier compaction");
        // once conditions are met call this function
        let _thread_count = self.config.max_compaction_threads;
        let _chunk_goal = self.config.target_chunks;
        if self.config.local_disk {} // mostly going to ignore this
        Ok(())
    }
    // ! add in update config function
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
