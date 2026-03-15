// BASE BRANCH Issue #21
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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
    InvalidCRC(Vec<u8>),              //crc that was there instead of the correct one
    InvalidFooter(String),            // footer places in wrong space?
    InvalidSparseIndex(),             // missing sparse index len or missing sparse index
    DataSectionError(DataSectionErr), // what ever other error will be this
}
#[derive(Debug)]
pub enum CompactionError {
    CompactionIoError(io::Error),
    InvalidTable(String, TableCorruption), //(file name,issue) -> should be able to continue but will tell caller
}
impl From<io::Error> for CompactionError {
    fn from(err: io::Error) -> Self {
        CompactionError::CompactionIoError(err)
    }
}

type CompactionResult<T> = Result<T, CompactionError>;

pub struct CompactionCoordinator {
    update_funcs: HashMap<CompactionEvents, Vec<Box<dyn FnMut() + Send + Sync>>>, // pass in functions to call once compaction even occures
    config: ComapctionCofig,
    compact_by: std::time::Instant,
}
impl CompactionCoordinator {
    pub const DRAINED_FILE_EXT: &str = ".drain";
    const TRANSITION_MERGED_SSTABLE_EXT: &str = ".tmp";
    const SS_TABLE_FILE_EXT: &str = ".bin";
    const BASE_DIRECTORY: &str = "data";
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
                    let data_dir = std::path::Path::new(Self::BASE_DIRECTORY);
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
                        let mut tables_with_created = vec![];
                        while let Some(entry) = read_dir.next_entry().await.unwrap() {
                            let file_name = entry.file_name();
                            if let Some(ext) = file_name.to_str().and_then(|s| s.split('.').last())
                            {
                                if ext == "bin" {
                                    small_ss_table_count += 1;
                                    let created =
                                        entry.metadata().await.unwrap().created().unwrap();
                                    tables_with_created
                                        .push((created, String::from(file_name.to_str().unwrap())));
                                }
                            }
                        }
                        // Newest files first so compaction processes recent SSTables before older ones.
                        tables_with_created.sort_by_key(|(created, _)| std::cmp::Reverse(*created));
                        let tables: Vec<String> = tables_with_created
                            .into_iter()
                            .map(|(_, file_name)| file_name)
                            .collect();

                        let mut lock = compaction_monitor.write().await;
                        if std::time::Instant::now() >= lock.compact_by
                            || small_ss_table_count > (lock.config.target_chunks as usize).pow(2)
                        {
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
    ) -> CompactionResult<()> {
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
            // Create units with target_chunks files (e.g., 4 files) , in order of newest -> oldest
            let unit_files = table_names[i..i + target_chunks].to_vec();
            compaction_units.push(CompactionUnit {
                target_files: unit_files,
                consumed_files: Vec::with_capacity(target_chunks),
            });
            i += target_chunks;
        }

        // Handle remaining files - only if we have at least 2
        let remaining = table_names.len() - i;
        // if not this file , save for next compaction cyle
        if remaining >= 2 {
            let unit_files = table_names[i..].to_vec();
            let capacity = unit_files.len();
            compaction_units.push(CompactionUnit {
                target_files: unit_files,
                consumed_files: Vec::with_capacity(capacity),
            });
        }

        // Compact each unit
        let mut compacted_tmp_files = Vec::new();
        for (id, unit) in compaction_units.iter_mut().enumerate() {
            match unit.compact(id, dir).await {
                Ok(result) => compacted_tmp_files.push(result),
                Err(e) => eprintln!("Compaction unit: {} failed: {:?}", id, e),
            }
        }
        // check for data/l_current+1 exist, if it does do nothing, otherwise create it

        let base = std::path::Path::new(Self::BASE_DIRECTORY);
        let next_level = base.join(format!("l{}", dir_level + 1));
        if !next_level.exists() {
            tokio::fs::create_dir(&next_level).await?;
        }

        //turn each .tmp into a .bin and move them to the next directory up
        for compacted_file in compacted_tmp_files.iter().take(1) {
            let new_name = compacted_file
                .replace(Self::TRANSITION_MERGED_SSTABLE_EXT, Self::SS_TABLE_FILE_EXT);

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
        }
        // build summary table (min,max,level wide bloom filter)
        self.build_level_summary().await?;
        // let interested parties know compaction has finished; mabey place in seperate function or in monitor
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
    async fn build_level_summary(&mut self) -> CompactionResult<()> {
        Ok(())
    }
}
#[derive(Debug)]
struct CompactionUnit {
    target_files: Vec<String>,
    consumed_files: Vec<String>,
}
enum TableType {
    SSTable(tokio::fs::File),
    RawDataSection(tokio::fs::File),
}
impl CompactionUnit {
    const HEADER_CRC_SIZE: usize = 64;
    const FOOTER_SIZE_FIELD_LEN: usize = 8;
    const SPARSE_INDEX_SIZE_FIELD_LEN: usize = 4;

    // output the final compacted file (must still be .tmp) include the directory
    async fn compact(&mut self, id: usize, dir: &PathBuf) -> CompactionResult<String> {
        if self.target_files.is_empty() {
            return Err(CompactionError::CompactionIoError(io::Error::new(
                io::ErrorKind::InvalidInput,
                "compaction unit has no files",
            )));
        }

        // Pairwise compaction rounds: [a,b,c] -> [ab_tmp, c] -> [abc_tmp]
        let mut pass = 0usize;

        while self.target_files.len() > 1 {
            let mut next_round: Vec<String> = Vec::with_capacity((self.target_files.len() + 1) / 2);
            let mut pair_idx = 0usize;

            while self.target_files.len() >= 2 {
                let left = self.target_files.remove(0);
                let right = self.target_files.remove(0);
                let out_name = format!("compaction_unit{}_pass{}_{}.tmp", id, pass, pair_idx);

                println!("output file name ::: {}", out_name);
                // join_tables internals are intentionally stubbed for now.
                // First pass inputs are .bin SSTables. Later pass inputs are .tmp raw data sections.
                let left_kind = Self::table_type_from_name(&left);
                let right_kind = Self::table_type_from_name(&right);
                let left_file = tokio::fs::File::open(dir.join(&left)).await?;
                let right_file = tokio::fs::File::open(dir.join(&right)).await?;
                let dest_file = tokio::fs::File::create(dir.join(&out_name)).await?;
                let left_table = match left_kind {
                    TableTypeKind::SSTable => TableType::SSTable(left_file),
                    TableTypeKind::RawDataSection => TableType::RawDataSection(left_file),
                };
                let right_table = match right_kind {
                    TableTypeKind::SSTable => TableType::SSTable(right_file),
                    TableTypeKind::RawDataSection => TableType::RawDataSection(right_file),
                };
                Self::join_tables(left_table, &left, right_table, &right, dest_file).await?;

                // Once a source table has been successfully joined, rename first, then mark consumed.
                let consumed_left = Self::mark_drained_file(dir, &left).await?;
                let consumed_right = Self::mark_drained_file(dir, &right).await?;
                self.consumed_files.push(consumed_left);
                self.consumed_files.push(consumed_right);

                next_round.push(out_name);
                pair_idx += 1;
            }

            // Odd number case: carry last file into the next pass unchanged.
            if let Some(odd_file) = self.target_files.pop() {
                next_round.push(odd_file);
            }

            self.target_files = next_round;
            pass += 1;
        }

        let data_section = self.target_files.pop().ok_or_else(|| {
            CompactionError::CompactionIoError(io::Error::new(
                io::ErrorKind::InvalidData,
                "compaction finished without output file",
            ))
        })?;

        let ds = tokio::fs::File::open(dir.join(&data_section)).await?;
        Self::build_from_data_section(ds).await?;
        // delete all consumed files
        Ok(data_section)
    }
    async fn join_tables(
        left: TableType,
        l_name: &str,
        right: TableType,
        r_name: &str,
        dest: tokio::fs::File,
    ) -> CompactionResult<()> {
        //validate the table first, crc, footer len, sparse index,
        // keep track of where it should end in memory (know when to stop iterating)
        let (l_file, l_size) = Self::to_data_section(l_name, left).await?;
        let (r_file, r_size) = Self::to_data_section(r_name, right).await?;
        // ! for now assume that the ss-tables are sorted, ill update this after a working version exist where we check
        // ! prev key < cur_kevy
        // ! only have four keys (at most) in ram at a time.
        // ! left_prev , right_prev, left_cur, right_cur this is to make sure it holds a sorted realationship
        let left_ptr = 0usize;
        let right_ptr = 0usize;

        while left_ptr < l_size && right_ptr < r_size {
            // always read left first
        }

        Ok(())
    }
    async fn build_from_data_section(source: tokio::fs::File) -> CompactionResult<()> {
        Ok(())
    }

    fn table_type_from_name(file_name: &str) -> TableTypeKind {
        if file_name.ends_with(CompactionCoordinator::SS_TABLE_FILE_EXT) {
            TableTypeKind::SSTable
        } else {
            TableTypeKind::RawDataSection
        }
    }
    // returns file ptr with the ptr at the start of the data section as well as
    // the size of the data section as the second field
    async fn to_data_section(
        name: &str,
        file: TableType,
    ) -> CompactionResult<(tokio::fs::File, usize)> {
        let ptr = match file {
            TableType::SSTable(mut f) => {
                // SS-table layout:
                // [CRC:64][footer_size:u64][bloom filter][sparse_index_size:u32][sparse_index][data_section][footer]
                //
                // The goal here is to reposition the file cursor at the first byte of the data section
                // and return the number of bytes that belong to the data section only.

                let file_len = f.metadata().await?.len() as usize;

                // 1. Validate the fixed-width CRC header.
                let mut crc_buff = [0u8; Self::HEADER_CRC_SIZE];
                f.read_exact(&mut crc_buff).await?;
                if crc_buff != super::disk::HEADER_CRC.as_bytes() {
                    return Err(CompactionError::InvalidTable(
                        String::from(name),
                        TableCorruption::InvalidCRC(crc_buff.to_vec()),
                    ));
                }

                // 2. Read the footer-size field written immediately after the CRC.
                let mut footer_buffer = [0u8; Self::FOOTER_SIZE_FIELD_LEN];
                f.read_exact(&mut footer_buffer).await?;
                let footer_size = u64::from_le_bytes(footer_buffer) as usize;
                if footer_size == 0 || footer_size > file_len {
                    return Err(CompactionError::InvalidTable(
                        String::from(name),
                        TableCorruption::InvalidFooter(format!(
                            "invalid footer size {footer_size} for file length {file_len}"
                        )),
                    ));
                }

                // 3. Skip the bloom filter and land on the sparse-index-size field.
                let sparse_index_len_start = Self::HEADER_CRC_SIZE
                    + Self::FOOTER_SIZE_FIELD_LEN
                    + super::disk::BLOOM_FILTER_SIZE;
                f.seek(std::io::SeekFrom::Start(sparse_index_len_start as u64))
                    .await?;

                // 4. Read the byte length of the sparse-index payload.
                let mut sparse_index_size_buf = [0u8; Self::SPARSE_INDEX_SIZE_FIELD_LEN];
                f.read_exact(&mut sparse_index_size_buf).await?;

                let sparse_index_size = u32::from_le_bytes(sparse_index_size_buf) as usize;
                let data_offset =
                    sparse_index_len_start + Self::SPARSE_INDEX_SIZE_FIELD_LEN + sparse_index_size;

                if data_offset > file_len || data_offset + footer_size > file_len {
                    return Err(CompactionError::InvalidTable(
                        String::from(name),
                        TableCorruption::InvalidSparseIndex(),
                    ));
                }

                // 5. The data section spans from `data_offset` up to the footer at EOF.
                let data_section_size = file_len - data_offset - footer_size;
                f.seek(std::io::SeekFrom::Start(data_offset as u64)).await?;

                (f, data_section_size)
            }
            TableType::RawDataSection(f) => {
                // Raw intermediate files are already just data-section bytes with no SSTable framing.
                let size = f.metadata().await?.len() as usize;
                (f, size)
            }
        };
        Ok(ptr)
    }

    async fn mark_drained_file(dir: &PathBuf, file_name: &str) -> CompactionResult<String> {
        if let Some(base_name) = file_name.strip_suffix(CompactionCoordinator::SS_TABLE_FILE_EXT) {
            let drained_name = format!("{}{}", base_name, CompactionCoordinator::DRAINED_FILE_EXT);
            let source = dir.join(file_name);
            let drained = dir.join(&drained_name);
            tokio::fs::rename(source, drained).await?;
            return Ok(drained_name);
        }

        Ok(file_name.to_string())
    }
    // if key len is 1 (u32) and value is 255 is a tombstone
    #[inline]
    fn is_tombstone(key: Vec<u8>, value: Vec<u8>) -> bool {
        key.len() == 1 && value.len() == 1 && value[0] == 255
    }
}

enum TableTypeKind {
    SSTable,
    RawDataSection,
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

#[cfg(test)]
mod compaction_unit_test {
    use super::*;

    const DIRECTORY: &str = "test-data";

    #[tokio::test]
    #[ignore = "fill in a real ss-table file name before running"]
    async fn to_data_section_reads_sstable_offsets() {
        let file_name = "ss-table-entry_5b99f04b-1272-4d1c-8f21-44522e12d3a4.bin";
        let path = std::path::Path::new(DIRECTORY).join(file_name);
        println!("path : {}", path.display());
        let file = tokio::fs::File::open(&path)
            .await
            .expect("fill in a valid ss-table path before running this test");

        let (mut data_section, data_section_size) = CompactionUnit::to_data_section(
            path.to_str().unwrap_or(file_name),
            TableType::SSTable(file),
        )
        .await
        .expect("expected valid ss-table layout");

        let cursor = data_section
            .stream_position()
            .await
            .expect("expected data-section cursor position");

        assert!(
            cursor > 0,
            "expected cursor to be positioned past the sstable metadata"
        );
        assert!(
            data_section_size > 0,
            "expected the extracted data section size to be greater than zero"
        );
    }
}
