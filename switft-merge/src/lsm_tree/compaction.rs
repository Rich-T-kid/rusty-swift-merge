// BASE BRANCH Issue #21
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::memtable::mem::ConfigInfo;
#[derive(PartialEq, Eq, Hash)]
pub enum CompactionEvents {
    Init,               // for config changes
    CompactionFinished, // ssTableReader needs to know to update indexes : (input ss-tables,output-sstables)
}
#[derive(Debug)]
pub enum TableCorruption {
    InvalidCRC(Vec<u8>),      //crc that was there instead of the correct one
    InvalidFooter(String),    // footer places in wrong space?
    InvalidSparseIndex(),     // missing sparse index len or missing sparse index
    DataSectionError(String), // what ever other error will be this
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
            match unit.compact(id, dir, dir_level + 1).await {
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

            let source_path = dir.join(compacted_file); // data/l1/test_drive.tmp

            let dest_path = next_level.join(&new_name); // data/l2/test_drive.bin

            println!(
                "Moving from {} to {}",
                source_path.display(),
                dest_path.display()
            );
            tokio::fs::rename(&source_path, &dest_path).await?;
        }
        // build summary table (min,max,level wide bloom filter)
        self.build_level_summary(dir_level + 1).await?;
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
    async fn build_level_summary(&mut self, level: u8) -> CompactionResult<()> {
        let level_dir = std::path::Path::new(Self::BASE_DIRECTORY).join(format!("l{}", level));
        if !level_dir.exists() {
            return Err(CompactionError::CompactionIoError(io::Error::new(
                io::ErrorKind::NotFound,
                format!("level directory missing: {}", level_dir.display()),
            )));
        }

        let mut read_dir = tokio::fs::read_dir(&level_dir).await?;
        let mut bin_files: Vec<String> = Vec::new();
        while let Some(entry) = read_dir.next_entry().await? {
            let file_name = entry.file_name();
            if let Some(name) = file_name.to_str() {
                if name.ends_with(Self::SS_TABLE_FILE_EXT) {
                    bin_files.push(name.to_string());
                }
            }
        }

        let mut min_key: Option<Vec<u8>> = None;
        let mut max_key: Option<Vec<u8>> = None;

        for file_name in &bin_files {
            let file = tokio::fs::File::open(level_dir.join(file_name)).await?;
            let (mut data_section, data_section_size) =
                CompactionUnit::to_data_section(file_name, TableType::SSTable(file)).await?;

            let mut ptr = 0usize;
            while ptr < data_section_size {
                let ((key, _value), bytes_read) =
                    CompactionUnit::read_data_section_entry(file_name, &mut data_section).await?;
                ptr += bytes_read;

                // ! Issue #9
                // TODO: Feed each key into the level-wide bloom filter builder.

                match &min_key {
                    Some(current) if key >= *current => {}
                    _ => min_key = Some(key.clone()),
                }
                match &max_key {
                    Some(current) if key <= *current => {}
                    _ => max_key = Some(key),
                }
            }
        }

        let min_u64 = min_key
            .as_ref()
            .map(|key| Self::key_to_u64(key))
            .unwrap_or(0);
        let max_u64 = max_key
            .as_ref()
            .map(|key| Self::key_to_u64(key))
            .unwrap_or(0);

        let bloom_size = super::disk::BLOOM_FILTER_SIZE * bin_files.len();
        let mut level_summary = Vec::with_capacity(16 + bloom_size);
        level_summary.extend_from_slice(&min_u64.to_le_bytes());
        level_summary.extend_from_slice(&max_u64.to_le_bytes());
        level_summary.extend_from_slice(&vec![0u8; bloom_size]);

        let level_filter_path = level_dir.join("level.filter");
        tokio::fs::write(level_filter_path, level_summary).await?;

        Ok(())
    }
    fn key_to_u64(key: &[u8]) -> u64 {
        // TODO(Issue #9): Replace this temporary hash marker with true key boundary encoding.
        // Deterministic 64-bit FNV-1a hash for compact min/max key markers.
        let mut hash = 0xcbf29ce484222325u64;
        for byte in key {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
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
    async fn compact(
        &mut self,
        id: usize,
        dir: &PathBuf,
        output_level: u8,
    ) -> CompactionResult<String> {
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
                let mut dest_file = tokio::fs::File::create(dir.join(&out_name)).await?;
                let left_table = match left_kind {
                    TableTypeKind::SSTable => TableType::SSTable(left_file),
                    TableTypeKind::RawDataSection => TableType::RawDataSection(left_file),
                };
                let right_table = match right_kind {
                    TableTypeKind::SSTable => TableType::SSTable(right_file),
                    TableTypeKind::RawDataSection => TableType::RawDataSection(right_file),
                };
                Self::join_tables(left_table, &left, right_table, &right, &mut dest_file).await?;

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

        let ds = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join(&data_section))
            .await?;
        Self::build_from_data_section(ds, output_level).await?;
        // Delete all consumed source files once the rebuilt output is in place.
        for consumed in self.consumed_files.drain(..) {
            let consumed_path = dir.join(&consumed);
            match tokio::fs::remove_file(&consumed_path).await {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(CompactionError::CompactionIoError(err)),
            }
        }
        Ok(data_section)
    }
    async fn join_tables(
        left: TableType,
        l_name: &str,
        right: TableType,
        r_name: &str,
        dest: &mut tokio::fs::File,
    ) -> CompactionResult<()> {
        //validate the table first, crc, footer len, sparse index,
        // keep track of where it should end in memory (know when to stop iterating)
        let (mut l_file, l_size) = Self::to_data_section(l_name, left).await?;
        let (mut r_file, r_size) = Self::to_data_section(r_name, right).await?;
        // ! for now assume that the ss-tables are sorted, ill update this after a working version exist where we check
        // ! prev key < cur_kevy
        // ! only have four keys (at most) in ram at a time.
        // ! left_prev , right_prev, left_cur, right_cur this is to make sure it holds a sorted realationship
        let mut left_ptr = 0usize;
        let mut right_ptr = 0usize;

        let mut left_entry =
            Self::read_next_non_tombstone_entry(l_name, &mut l_file, &mut left_ptr, l_size).await?;

        let mut right_entry =
            Self::read_next_non_tombstone_entry(r_name, &mut r_file, &mut right_ptr, r_size)
                .await?;

        while left_entry.is_some() && right_entry.is_some() {
            let ordering = {
                let (left_key, _) = left_entry.as_ref().unwrap();
                let (right_key, _) = right_entry.as_ref().unwrap();
                left_key.cmp(right_key)
            };

            match ordering {
                std::cmp::Ordering::Less => {
                    let (left_key, left_val) = left_entry.take().unwrap();
                    Self::write_data_section_entry(dest, &left_key, &left_val).await?;

                    left_entry = Self::read_next_non_tombstone_entry(
                        l_name,
                        &mut l_file,
                        &mut left_ptr,
                        l_size,
                    )
                    .await?;
                }
                std::cmp::Ordering::Greater => {
                    let (right_key, right_val) = right_entry.take().unwrap();
                    Self::write_data_section_entry(dest, &right_key, &right_val).await?;

                    right_entry = Self::read_next_non_tombstone_entry(
                        r_name,
                        &mut r_file,
                        &mut right_ptr,
                        r_size,
                    )
                    .await?;
                }
                std::cmp::Ordering::Equal => {
                    // Equal keys: prefer the newer record (left input), and advance both sides.
                    let (left_key, left_val) = left_entry.take().unwrap();
                    let _ = right_entry.take().unwrap();
                    Self::write_data_section_entry(dest, &left_key, &left_val).await?;

                    left_entry = Self::read_next_non_tombstone_entry(
                        l_name,
                        &mut l_file,
                        &mut left_ptr,
                        l_size,
                    )
                    .await?;
                    right_entry = Self::read_next_non_tombstone_entry(
                        r_name,
                        &mut r_file,
                        &mut right_ptr,
                        r_size,
                    )
                    .await?;
                }
            }
        }

        while let Some((left_key, left_val)) = left_entry.take() {
            Self::write_data_section_entry(dest, &left_key, &left_val).await?;
            left_entry =
                Self::read_next_non_tombstone_entry(l_name, &mut l_file, &mut left_ptr, l_size)
                    .await?;
        }

        while let Some((right_key, right_val)) = right_entry.take() {
            Self::write_data_section_entry(dest, &right_key, &right_val).await?;
            right_entry =
                Self::read_next_non_tombstone_entry(r_name, &mut r_file, &mut right_ptr, r_size)
                    .await?;
        }

        Ok(())
    }
    async fn read_prefixed_bytes(
        file_name: &str,
        file_ptr: &mut tokio::fs::File,
    ) -> CompactionResult<(Vec<u8>, usize)> {
        let mut size_buffer = [0u8; 4];
        file_ptr.read_exact(&mut size_buffer).await?;
        let value_size = u32::from_le_bytes(size_buffer) as usize;
        let mut value_buffer = vec![0u8; value_size];
        file_ptr.read_exact(&mut value_buffer).await?;

        if value_buffer.len() != value_size {
            return Err(CompactionError::InvalidTable(
                String::from(file_name),
                TableCorruption::DataSectionError(
                    "length prefix did not match decoded data size".to_string(),
                ),
            ));
        }

        Ok((value_buffer, 4 + value_size))
    }
    async fn read_data_section_entry(
        file_name: &str,
        file_ptr: &mut tokio::fs::File,
    ) -> CompactionResult<((Vec<u8>, Vec<u8>), usize)> {
        let (key, key_bytes) = Self::read_prefixed_bytes(file_name, file_ptr).await?;
        let (value, value_bytes) = Self::read_prefixed_bytes(file_name, file_ptr).await?;
        Ok(((key, value), key_bytes + value_bytes))
    }
    async fn read_next_non_tombstone_entry(
        file_name: &str,
        file_ptr: &mut tokio::fs::File,
        ptr: &mut usize,
        section_size: usize,
    ) -> CompactionResult<Option<(Vec<u8>, Vec<u8>)>> {
        while *ptr < section_size {
            let ((key, value), bytes_read) =
                Self::read_data_section_entry(file_name, file_ptr).await?;
            *ptr += bytes_read;

            if Self::is_tombstone(key.clone(), value.clone()) {
                continue;
            }

            return Ok(Some((key, value)));
        }

        Ok(None)
    }
    async fn write_data_section_entry(
        dest: &mut tokio::fs::File,
        key: &[u8],
        value: &[u8],
    ) -> CompactionResult<()> {
        let key_len = u32::try_from(key.len()).map_err(|_| {
            CompactionError::InvalidTable(
                "compaction-output".to_string(),
                TableCorruption::DataSectionError("key length exceeds u32".to_string()),
            )
        })?;
        let value_len = u32::try_from(value.len()).map_err(|_| {
            CompactionError::InvalidTable(
                "compaction-output".to_string(),
                TableCorruption::DataSectionError("value length exceeds u32".to_string()),
            )
        })?;

        dest.write_all(&key_len.to_le_bytes()).await?;
        dest.write_all(key).await?;
        dest.write_all(&value_len.to_le_bytes()).await?;
        dest.write_all(value).await?;
        Ok(())
    }
    async fn build_from_data_section(
        mut source: tokio::fs::File,
        level: u8,
    ) -> CompactionResult<()> {
        source.seek(std::io::SeekFrom::Start(0)).await?;
        let mut data_section = Vec::new();
        source.read_to_end(&mut data_section).await?;

        let block_stride = 64 * 1024 * std::cmp::max(1usize, level as usize); // 64kb * level -> larger levels have more data so index becomes more sparse
        let mut sparse_index: Vec<(u64, Vec<u8>)> = Vec::new();
        let mut max_key: Vec<u8> = Vec::new();

        let mut cursor = 0usize;
        let mut bytes_since_sparse_mark = 0usize;
        while cursor < data_section.len() {
            let entry_start = cursor;

            if cursor + 4 > data_section.len() {
                return Err(CompactionError::CompactionIoError(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid key length prefix in data section",
                )));
            }
            let key_len =
                u32::from_le_bytes(data_section[cursor..cursor + 4].try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid key length bytes")
                })?) as usize;
            cursor += 4;

            if cursor + key_len > data_section.len() {
                return Err(CompactionError::CompactionIoError(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "key length exceeds remaining data section bytes",
                )));
            }
            let key = data_section[cursor..cursor + key_len].to_vec();
            cursor += key_len;

            if cursor + 4 > data_section.len() {
                return Err(CompactionError::CompactionIoError(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid value length prefix in data section",
                )));
            }
            let value_len =
                u32::from_le_bytes(data_section[cursor..cursor + 4].try_into().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid value length bytes")
                })?) as usize;
            cursor += 4;

            if cursor + value_len > data_section.len() {
                return Err(CompactionError::CompactionIoError(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "value length exceeds remaining data section bytes",
                )));
            }
            cursor += value_len;

            let traversed_entry_bytes = cursor - entry_start;
            if sparse_index.is_empty() || bytes_since_sparse_mark >= block_stride {
                sparse_index.push((entry_start as u64, key.clone()));
                bytes_since_sparse_mark = 0;
            }
            bytes_since_sparse_mark += traversed_entry_bytes;
            max_key = key;
        }

        let mut sparse_index_bytes = Vec::new();
        for (offset, key) in &sparse_index {
            let key_len = u32::try_from(key.len()).map_err(|_| {
                CompactionError::CompactionIoError(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sparse index key length exceeds u32",
                ))
            })?;
            sparse_index_bytes.extend_from_slice(&key_len.to_le_bytes());
            sparse_index_bytes.extend_from_slice(key);
            sparse_index_bytes.extend_from_slice(&offset.to_le_bytes());
        }

        let mut footer = Vec::new();
        let max_key_len = u32::try_from(max_key.len()).map_err(|_| {
            CompactionError::CompactionIoError(io::Error::new(
                io::ErrorKind::InvalidData,
                "max key length exceeds u32",
            ))
        })?;
        footer.extend_from_slice(&max_key_len.to_le_bytes());
        footer.extend_from_slice(&max_key);

        let trailing_bloom = vec![0u8; super::disk::BLOOM_FILTER_SIZE];
        let footer_size = u64::try_from(footer.len() + trailing_bloom.len()).map_err(|_| {
            CompactionError::CompactionIoError(io::Error::new(
                io::ErrorKind::InvalidData,
                "footer size exceeds u64",
            ))
        })?;
        let sparse_index_size = u32::try_from(sparse_index_bytes.len()).map_err(|_| {
            CompactionError::CompactionIoError(io::Error::new(
                io::ErrorKind::InvalidData,
                "sparse index size exceeds u32",
            ))
        })?;

        let mut output = Vec::new();
        output.extend_from_slice(super::disk::HEADER_CRC.as_bytes());
        output.extend_from_slice(&footer_size.to_le_bytes());
        // Reserved bloom-filter bytes after footer-size field.
        output.extend_from_slice(&vec![0u8; super::disk::BLOOM_FILTER_SIZE]);
        output.extend_from_slice(&sparse_index_size.to_le_bytes());
        output.extend_from_slice(&sparse_index_bytes);
        output.extend_from_slice(&data_section);
        output.extend_from_slice(&footer);
        // Reserved bloom-filter bytes appended after footer for later bloom work.
        output.extend_from_slice(&trailing_bloom);

        source.set_len(0).await?;
        source.seek(std::io::SeekFrom::Start(0)).await?;
        source.write_all(&output).await?;
        source.flush().await?;
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
    use std::path::Path;

    const DIRECTORY: &str = "test-data";

    async fn pick_two_sstable_paths() -> (std::path::PathBuf, std::path::PathBuf) {
        let mut read_dir = tokio::fs::read_dir(DIRECTORY)
            .await
            .expect("expected test-data directory to exist");
        let mut paths: Vec<std::path::PathBuf> = Vec::new();

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .expect("expected test-data directory to be readable")
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("bin") {
                paths.push(path);
            }
        }

        paths.sort();
        assert!(
            paths.len() >= 2,
            "expected at least two .bin SSTables in test-data, found {}",
            paths.len()
        );
        (paths[0].clone(), paths[1].clone())
    }

    #[tokio::test]
    async fn join_tables_file1_file2_output_is_sorted() {
        let (file1_path, file2_path) = pick_two_sstable_paths().await;
        let file1_source = file1_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("expected utf-8 file1 name")
            .to_string();
        let file2_source = file2_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("expected utf-8 file2 name")
            .to_string();

        println!("file1 source: {}", file1_source);
        println!("file2 source: {}", file2_source);

        let output_path = Path::new(DIRECTORY).join("join-tables-output.tmp");
        if output_path.exists() {
            tokio::fs::remove_file(&output_path)
                .await
                .expect("expected old output to be removable");
        }

        let mut dest = tokio::fs::File::create(&output_path)
            .await
            .expect("expected destination file to be created");

        CompactionUnit::join_tables(
            TableType::SSTable(
                tokio::fs::File::open(&file1_path)
                    .await
                    .expect("expected file1 to open"),
            ),
            "file1",
            TableType::SSTable(
                tokio::fs::File::open(&file2_path)
                    .await
                    .expect("expected file2 to open"),
            ),
            "file2",
            &mut dest,
        )
        .await
        .expect("expected join_tables to complete");
        drop(dest);

        let mut out_file = tokio::fs::File::open(&output_path)
            .await
            .expect("expected merged output to open");
        let out_size = out_file
            .metadata()
            .await
            .expect("expected merged output metadata")
            .len() as usize;

        let mut ptr = 0usize;
        let mut keys: Vec<String> = Vec::new();
        while ptr < out_size {
            let ((key, value), bytes_read) =
                CompactionUnit::read_data_section_entry("join-tables-output.tmp", &mut out_file)
                    .await
                    .expect("expected merged output entry to be readable");
            ptr += bytes_read;
            let printable_key =
                String::from_utf8(key.clone()).expect("expected ascii superhero key");
            println!("merged key: {}", printable_key);

            assert!(
                !CompactionUnit::is_tombstone(key, value),
                "merged output contains tombstone for key: {}",
                printable_key
            );

            keys.push(printable_key);
        }

        assert_eq!(ptr, out_size, "expected to consume the full merged output");

        for pair in keys.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "expected sorted merged keys, but {:?} came before {:?}",
                pair[0],
                pair[1]
            );
        }

        tokio::fs::remove_file(&output_path)
            .await
            .expect("expected merged output to be removable");
    }

    #[tokio::test]
    async fn join_tables_writes_named_result_file() {
        let file1 = "ss-table-entry_53dc8580-214d-47e9-836d-0f9314646c70.bin";
        let file2 = "ss-table-entry_ed7076c2-e0c2-4ab1-a1d0-f0e46802a980.bin";
        let file1_path = Path::new(DIRECTORY).join(file1);
        let file2_path = Path::new(DIRECTORY).join(file2);
        let output_path = Path::new(DIRECTORY).join("join_ss-table_result");

        if output_path.exists() {
            tokio::fs::remove_file(&output_path)
                .await
                .expect("expected old join_ss-table_result to be removable");
        }

        let mut dest = tokio::fs::File::create(&output_path)
            .await
            .expect("expected join_ss-table_result to be created");

        CompactionUnit::join_tables(
            TableType::SSTable(
                tokio::fs::File::open(&file1_path)
                    .await
                    .expect("expected file1 to open"),
            ),
            "file1",
            TableType::SSTable(
                tokio::fs::File::open(&file2_path)
                    .await
                    .expect("expected file2 to open"),
            ),
            "file2",
            &mut dest,
        )
        .await
        .expect("expected join_tables to write result file");
        drop(dest);

        let size = tokio::fs::metadata(&output_path)
            .await
            .expect("expected output metadata")
            .len();
        assert!(size > 0, "expected join_ss-table_result to be non-empty");

        println!("wrote merged output to {}", output_path.display());
    }

    #[tokio::test]
    async fn build_sstable_on_joined_result_file() {
        let output_path = Path::new(DIRECTORY).join("join_ss-table_result");
        assert!(
            output_path.exists(),
            "expected join_ss-table_result to exist before building sstable"
        );

        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&output_path)
            .await
            .expect("expected join_ss-table_result to open");

        CompactionUnit::build_from_data_section(file, 2)
            .await
            .expect("expected sstable build to succeed on join_ss-table_result");

        let size = tokio::fs::metadata(&output_path)
            .await
            .expect("expected rebuilt output metadata")
            .len();
        assert!(
            size > 0,
            "expected rebuilt join_ss-table_result to remain non-empty"
        );

        println!("rebuilt sstable format at {}", output_path.display());
    }
}
