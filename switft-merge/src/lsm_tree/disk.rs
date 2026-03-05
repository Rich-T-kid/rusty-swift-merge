use std::collections::HashMap;
use std::error::Error;
use std::fmt::Display;
use std::fs;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::sync::RwLock;

use crate::memtable::mem::TableEntry;
pub const HEADER_CRC: &str = "054a62a514e1d7d93b2955772fe6070d03a9d58f34a42d85918ac975488dbbe4";
const BLOOM_FILTER_SIZE: usize = 1000;
pub const PAGE_PER_BLOCK: usize = 4;

#[derive(Debug)]
pub enum LsmTreeError {
    IOErr(io::Error),
    UnknownErr(Box<dyn std::error::Error>),
    InitFailure(String),
}
type SearchMetaData = (u16, u8); // u16: number of ss-tables searched, u8: number of levels searched
pub enum SearchResult {
    Found(Vec<u8>, SearchMetaData), // Vec<u8>: value  | read below
    Missing(SearchMetaData), //Vec<u8>: Key searched for ! caller already has this info tho so i removed the key
}
impl Display for LsmTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LsmTreeError::IOErr(err) => write!(f, "IO error: {}", err),
            LsmTreeError::UnknownErr(err) => write!(f, "Unknown error: {}", err),
            LsmTreeError::InitFailure(msg) => write!(f, "Initialization failure: {}", msg),
        }
    }
}
impl Error for LsmTreeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LsmTreeError::IOErr(err) => Some(err),
            LsmTreeError::UnknownErr(err) => Some(err.as_ref()),
            LsmTreeError::InitFailure(_) => None,
        }
    }
}
impl From<std::io::Error> for LsmTreeError {
    fn from(value: std::io::Error) -> Self {
        LsmTreeError::IOErr(value)
    }
}
impl Display for LsmTreeReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Levels: {}", self.level_array.len())?;

        for (level_idx, file_map) in self.level_array.iter().enumerate() {
            writeln!(f, "\nLevel {}:", level_idx + 1)?;
            writeln!(f, "  SS-Tables: {}", file_map.len())?;

            if file_map.is_empty() {
                continue;
            }

            // Get all entries sorted by timestamp
            let mut entries: Vec<_> = file_map.keys().collect();
            entries.sort_by_key(|metadata| metadata.creation_timestamp);

            // Display first entry
            if let Some(metadata) = entries.first() {
                let first_key = metadata
                    .sparse_index
                    .first()
                    .map(|(_, key)| String::from_utf8_lossy(key).to_string())
                    .unwrap_or_else(|| "N/A".to_string());
                writeln!(f, "  First SS-Table (oldest):")?;
                writeln!(f, "    Timestamp: {}", metadata.creation_timestamp)?;
                writeln!(f, "    First Key: {}", first_key)?;
                writeln!(
                    f,
                    "    Max Key: {}",
                    String::from_utf8_lossy(&metadata.max_key)
                )?;
                writeln!(
                    f,
                    "    Sparse Index Entries: {}",
                    metadata.sparse_index.len()
                )?;
            }

            // Display last entry if more than one exists
            if entries.len() > 1 {
                if let Some(metadata) = entries.last() {
                    let first_key = metadata
                        .sparse_index
                        .first()
                        .map(|(_, key)| String::from_utf8_lossy(key).to_string())
                        .unwrap_or_else(|| "N/A".to_string());
                    writeln!(f, "  Last SS-Table (newest):")?;
                    writeln!(f, "    Timestamp: {}", metadata.creation_timestamp)?;
                    writeln!(f, "    First Key: {}", first_key)?;
                    writeln!(
                        f,
                        "    Max Key: {}",
                        String::from_utf8_lossy(&metadata.max_key)
                    )?;
                    writeln!(
                        f,
                        "    Sparse Index Entries: {}",
                        metadata.sparse_index.len()
                    )?;
                }
            }
        }

        Ok(())
    }
}
type LevelIndex = (Vec<(u64, Vec<u8>)>, u64); // (sparse_index, timestamp)
type FileMap = HashMap<SSTableMetaData, TableReader>;

#[derive(Debug)]
pub struct LsmTreeReader {
    /*
    for each level 0-N there is a index that provides useful metadata about each ss-table
    the level index provides a file pointer as well as the sparse index for that ss-table
    the value of this level index is a table reader which provides a high level abstraction of reading these ss-tables

    0. read request
    1.start from level 0 of level array
    2.iteratre through hashmap to see which LevelIndex(sparse index) have a valid range for the key we are looking for
    3. compile these LevelIndexes
    4. Sort these compiled LevelIndex by the order of creation -> newest files are searched first
    5.A check the bloom filter if no matches is found ignore file. (For now ignore this step)
    4.B using the spark index begin seaching through each file
    4.C if result is found
    4.D if result is not found move onto next file
    5. If no corresponding key is found in this level, move up to the next level
    6. repeat until each level has been searched
    7. if not found return None to caller
     */
    level_array: Vec<FileMap>,
}

impl LsmTreeReader {
    // works within /data directory
    // read only
    //
    pub fn new() -> Result<Self, LsmTreeError> {
        let data_dir = std::path::Path::new("data");

        // Check if data directory exists, return empty if it doesn't (no data yet is okay)
        if !data_dir.exists() {
            return Ok(Self {
                level_array: Vec::new(),
            });
        }

        let mut level_array: Vec<FileMap> = Vec::new();

        // Read each level directory starting from l1
        let mut level_num = 1;
        loop {
            let level_dir = data_dir.join(format!("l{}", level_num));

            // Stop when we can't find the next level directory
            if !level_dir.exists() {
                break;
            }

            let mut file_map: FileMap = HashMap::new();

            // Read all files in this level directory
            let entries = fs::read_dir(&level_dir).map_err(|e| LsmTreeError::IOErr(e))?;

            for entry in entries {
                let entry = entry.map_err(|e| LsmTreeError::IOErr(e))?;
                let path = entry.path();

                // Only process .bin files
                if path.extension().and_then(|s| s.to_str()) == Some("bin") {
                    // Open file for TableReader
                    let file = fs::File::open(&path).map_err(|e| LsmTreeError::IOErr(e))?;

                    let mut table_reader =
                        TableReader::new(file).map_err(|e| LsmTreeError::IOErr(e))?;

                    let crc = table_reader.read_header()?;
                    if crc != HEADER_CRC.as_bytes() {
                        println!(
                            "{:?} contains an invalid crc header {:?}, skipping file",
                            path, crc
                        );
                        continue;
                    }

                    // Generate index metadata
                    let metadata = table_reader
                        .generate_index()
                        .map_err(|e| LsmTreeError::IOErr(e))?;

                    // Insert with SSTableMetaData as key
                    file_map.insert(metadata, table_reader);
                }
            }

            // Add this level's FileMap to the level_array
            level_array.push(file_map);
            level_num += 1;
        }

        Ok(Self { level_array })
    }

    /*
       newest (lower digit levels) ss-tables have the latest information so these will be searched first
       1. create a list of valid TableReader, iterate through all table Readers in sorted order (creation time newest to latest)
       2. this will be based on weather or not the key falls inbetween the first and last element of the sparse index (currently how this is set up the last written sparse index entry doesnt mean its the highest key as its written only in chunks, in the future well add some metadata to the footer about the largest key & decode it on read)
       3. if this falls inbeween the valid range check the bloom filter
       4. if the bloom filter doesnt return a confirmed No add it to the list
       5. iterate through the list and call TableReader.Search(key)
       6. if key is found return Ok(SearchResult::Found(value_bytes,SearchMetaData{ss-tables_read: u16, levels_searched: u8}))
    */
    pub fn read(&self, key: &[u8]) -> Result<SearchResult, LsmTreeError> {
        let mut levels_searched = 0u8;
        let mut ss_tables_searched = 0u16;

        // Iterate through each level (level 1, level 2, etc.)
        for file_map in &self.level_array {
            levels_searched += 1;

            // Collect valid table readers with their metadata, sorted by creation time (newest first)
            let mut valid_tables: Vec<(&SSTableMetaData, &TableReader)> = Vec::new();

            for (metadata, table_reader) in file_map.iter() {
                // Check if key falls within the range of this SS-table
                // First key from sparse index
                let first_key = metadata
                    .sparse_index
                    .first()
                    .map(|(_, k)| k.as_slice())
                    .unwrap_or(&[]);

                // Max key from footer metadata
                let max_key = metadata.max_key.as_slice();

                // Check if key is within range: first_key <= key <= max_key
                if key >= first_key && key <= max_key {
                    // TODO: Step 3 & 4 - Check bloom filter (skipped for now as per Issue #9)
                    // For now, we add all tables within range
                    valid_tables.push((metadata, table_reader));
                }
            }

            // Sort by creation timestamp (newest first)
            valid_tables.sort_by(|a, b| b.0.creation_timestamp.cmp(&a.0.creation_timestamp));

            // Search through valid tables
            for (_metadata, table_reader) in valid_tables {
                ss_tables_searched += 1;

                match table_reader.search(key) {
                    Ok(Some(value)) => {
                        // Found the key
                        return Ok(SearchResult::Found(
                            value,
                            (ss_tables_searched, levels_searched),
                        ));
                    }
                    Ok(None) => {
                        // Key not found in this table, continue to next
                        continue;
                    }
                    Err(e) => {
                        // IO error occurred
                        return Err(LsmTreeError::IOErr(e));
                    }
                }
            }
        }

        // Key not found in any level
        Ok(SearchResult::Missing((ss_tables_searched, levels_searched)))
    }
}
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct SSTableMetaData {
    creation_timestamp: u64,
    sparse_index: Vec<(u64, Vec<u8>)>,
    max_key: Vec<u8>,
}

type TableResult<T> = Result<T, io::Error>;
#[derive(Debug)]
struct TableReader {
    f: RwLock<fs::File>, // Changed: Use RwLock instead of RefCell for thread safety
    data_section_offset: Option<usize>,
    metadata_cache: Option<SSTableMetaData>,
}

impl TableReader {
    const HEADER_SIZE: usize = 64;
    const FOOTER_SIZE_OFFSET: usize = 64;
    const BLOOM_FILTER_SIZE: usize = BLOOM_FILTER_SIZE;

    fn new(f: fs::File) -> TableResult<Self> {
        Ok(Self {
            f: RwLock::new(f), // Changed: Wrap in RwLock
            data_section_offset: None,
            metadata_cache: None,
        })
    }

    fn read_header(&mut self) -> TableResult<Vec<u8>> {
        let mut file = self
            .f
            .write()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        file.seek(io::SeekFrom::Start(0))?;

        let mut buffer = [0u8; Self::HEADER_SIZE];
        file.read_exact(&mut buffer)?;
        Ok(buffer.to_vec())
    }

    fn get_bloom_filter(&mut self) -> TableResult<Vec<u8>> {
        let mut file = self
            .f
            .write()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        file.seek(io::SeekFrom::Start(Self::HEADER_SIZE as u64))?;

        let mut buffer = vec![0u8; Self::BLOOM_FILTER_SIZE];
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    fn read_footer(&mut self) -> TableResult<Vec<u8>> {
        let mut file = self
            .f
            .write()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        file.seek(io::SeekFrom::Start(Self::FOOTER_SIZE_OFFSET as u64))?;
        let mut footer_size_buf = [0u8; 8];
        file.read_exact(&mut footer_size_buf)?;
        let footer_size = u64::from_le_bytes(footer_size_buf) as i64;

        file.seek(io::SeekFrom::End(-footer_size))?;

        let mut key_len_buf = [0u8; 4];
        file.read_exact(&mut key_len_buf)?;
        let max_key_len = u32::from_le_bytes(key_len_buf) as usize;

        let mut max_key = vec![0u8; max_key_len];
        file.read_exact(&mut max_key)?;

        Ok(max_key)
    }

    fn generate_index(&mut self) -> TableResult<SSTableMetaData> {
        if let Some(ref cached) = self.metadata_cache {
            return Ok(cached.clone());
        }

        self.get_sparse_index()?;
        Ok(self.metadata_cache.as_ref().unwrap().clone())
    }

    fn get_sparse_index(&mut self) -> TableResult<Vec<(u64, Vec<u8>)>> {
        if let Some(ref cached) = self.metadata_cache {
            return Ok(cached.sparse_index.clone());
        }

        let mut file = self
            .f
            .write()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        let sparse_index_start = (Self::HEADER_SIZE + 8 + Self::BLOOM_FILTER_SIZE) as u64;
        file.seek(io::SeekFrom::Start(sparse_index_start))?;

        let mut size_buf = [0u8; 4];
        file.read_exact(&mut size_buf)?;
        let sparse_index_content_size = u32::from_le_bytes(size_buf) as usize;

        self.data_section_offset =
            Some(sparse_index_start as usize + 4 + sparse_index_content_size);

        let mut sparse_index_buffer = vec![0u8; sparse_index_content_size];
        file.read_exact(&mut sparse_index_buffer)?;

        drop(file); // Release the lock

        let mut result = Vec::new();
        let mut pos = 0;

        while pos < sparse_index_buffer.len() {
            let key_len = u32::from_le_bytes(
                sparse_index_buffer[pos..pos + 4]
                    .try_into()
                    .expect("slice length mismatch"),
            ) as usize;
            pos += 4;

            let key = sparse_index_buffer[pos..pos + key_len].to_vec();
            pos += key_len;

            let offset = u64::from_le_bytes(
                sparse_index_buffer[pos..pos + 8]
                    .try_into()
                    .expect("slice length mismatch"),
            );
            pos += 8;

            result.push((offset, key));
        }

        let max_key = self.read_footer()?;

        let metadata = self
            .f
            .read()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?
            .metadata()?;
        let created = metadata.created()?;
        let creation_timestamp = created
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
            .as_secs();

        let ss_table_metadata = SSTableMetaData {
            creation_timestamp,
            sparse_index: result.clone(),
            max_key,
        };
        self.metadata_cache = Some(ss_table_metadata);

        Ok(result)
    }

    fn search(&self, key: &[u8]) -> TableResult<Option<Vec<u8>>> {
        let sparse_index = self.metadata_cache.as_ref().unwrap().sparse_index.clone();

        let data_offset = self
            .data_section_offset
            .expect("data_section_offset should be set after get_sparse_index");

        let block_idx =
            sparse_index.binary_search_by(|(_, block_key)| block_key.as_slice().cmp(key));

        let search_offset = match block_idx {
            Ok(idx) => sparse_index[idx].0,
            Err(0) => return Ok(None),
            Err(idx) => sparse_index[idx - 1].0,
        };

        let file_position = data_offset as u64 + search_offset;
        let mut file = self
            .f
            .write()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        file.seek(io::SeekFrom::Start(file_position))?;

        loop {
            let mut key_len_buf = [0u8; 4];
            if file.read_exact(&mut key_len_buf).is_err() {
                return Ok(None);
            }
            let entry_key_len = u32::from_le_bytes(key_len_buf) as usize;

            let mut entry_key = vec![0u8; entry_key_len];
            file.read_exact(&mut entry_key)?;

            let mut value_len_buf = [0u8; 4];
            file.read_exact(&mut value_len_buf)?;
            let value_len = u32::from_le_bytes(value_len_buf) as usize;

            let mut value = vec![0u8; value_len];
            file.read_exact(&mut value)?;

            match entry_key.as_slice().cmp(key) {
                std::cmp::Ordering::Equal => return Ok(Some(value)),
                std::cmp::Ordering::Greater => return Ok(None),
                std::cmp::Ordering::Less => continue,
            }
        }
    }
}

/*
Issue #9
*/
pub struct BloomGenerator {}
impl BloomGenerator {
    pub fn generate_filter(
        _data: &std::collections::BTreeMap<Vec<u8>, Option<TableEntry>>,
        _bloom_false_positive_rate: f64,
    ) -> [u8; BLOOM_FILTER_SIZE] {
        // the size needs to be very strict should be BLOOM_FILTER_SIZE
        [0u8; 1000]
    }
    // returns a firm no , doesnt exist or a probably exist
    fn probably_exist(_bloom: [u8; BLOOM_FILTER_SIZE], _key: Vec<u8>) -> bool {
        // ! TODO
        true
    }
}

// Note: These tests depend on the SS-table file generated by running
// test_flush_superhero_entries() in memtable/mem.rs
mod table_reader_test {
    use super::*;
    const DIRECTORY: &str = "src/lsm_tree";
    const TEST_FILE: &str = "super_hero.bin";

    #[test]
    fn test_read_header() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();
        let header = reader.read_header().unwrap();
        assert_eq!(header, HEADER_CRC.as_bytes())
    }

    #[test]
    fn test_get_bloom_filter() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();
        let bloom_filter = reader.get_bloom_filter().unwrap();

        // Assert correct length
        assert_eq!(bloom_filter.len(), BLOOM_FILTER_SIZE);

        // Assert all bytes are zero
        assert!(
            bloom_filter.iter().all(|&b| b == 0),
            "Bloom filter should be all zeros"
        );
    }

    #[test]
    fn test_get_sparse_index() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();
        let sparse_index = reader.get_sparse_index().unwrap();

        // Expected sparse index based on superhero test
        let expected = vec![
            (0, vec![98, 108, 97, 99, 107, 95, 119, 105, 100, 111, 119]), // "black_widow"
            (60133, vec![116, 104, 111, 114]),                            // "thor"
        ];

        assert_eq!(sparse_index, expected, "Sparse index mismatch");
    }

    #[test]
    fn test_search_existing_key() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();

        // Search for a key that exists - "hulk"
        let result = reader.search(b"hulk").unwrap();
        assert!(result.is_some(), "Expected to find 'hulk' in SS-table");

        // Verify the value is the correct size (10KB entry serialized)
        let value = result.unwrap();
        assert!(value.len() > 0, "Expected non-empty value for 'hulk'");

        println!("Successfully found 'hulk' with value size: {}", value.len());
    }

    #[test]
    fn test_search_nonexistent_key_before_range() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();

        // Search for a key that comes before all superhero names alphabetically
        let result = reader.search(b"aardvark").unwrap();

        assert!(
            result.is_none(),
            "Expected None for key 'aardvark' that doesn't exist (before range)"
        );
        println!("Correctly returned None for non-existent key before range");
    }

    #[test]
    fn test_search_nonexistent_key_in_range() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();

        // Search for a key between existing keys - "green_lantern" falls between "doctor_strange" and "hawkeye"
        let result = reader.search(b"green_lantern").unwrap();

        assert!(
            result.is_none(),
            "Expected None for key 'green_lantern' that doesn't exist (in range)"
        );
        println!("Correctly returned None for non-existent key within range");
    }

    #[test]
    fn test_search_nonexistent_key_after_range() {
        let dir_path = std::path::Path::new(DIRECTORY).join(TEST_FILE);
        let mut reader = TableReader::new(fs::File::open(dir_path).unwrap()).unwrap();

        // Search for a key that comes after all superhero names alphabetically
        let result = reader.search(b"wonder_woman").unwrap();

        assert!(
            result.is_none(),
            "Expected None for key 'wonder_woman' that doesn't exist (after range)"
        );
        println!("Correctly returned None for non-existent key after range");
    }
}

mod lsm_reader_test {
    use crate::lsm_tree::disk::LsmTreeReader;

    use super::*;

    #[test]
    fn test_new() {
        let r = LsmTreeReader::new().unwrap();
        println!("{}", r);
        let size = r.level_array.len();
        println!("size: {size}")
    }

    #[test]
    fn test_read_existing_keys() {
        let mut reader = LsmTreeReader::new().unwrap();

        let test_keys = vec!["spider_man", "iron_man", "captain_america"];

        for key in test_keys {
            println!("\n=== Searching for existing key: '{}' ===", key);
            match reader.read(key.as_bytes()) {
                Ok(SearchResult::Found(value, (ss_tables, levels))) => {
                    println!("  Found key '{}'", key);
                    println!("  Value size: {} bytes", value.len());
                    println!("  SS-tables searched: {}", ss_tables);
                    println!("  Levels searched: {}", levels);
                }
                Ok(SearchResult::Missing((ss_tables, levels))) => {
                    println!(" Key '{}' not found (unexpected!)", key);
                    println!("  SS-tables searched: {}", ss_tables);
                    println!("  Levels searched: {}", levels);
                }
                Err(e) => {
                    println!("✗ Error searching for '{}': {}", key, e);
                }
            }
        }
    }

    #[test]
    fn test_read_nonexistent_keys() {
        let mut reader = LsmTreeReader::new().unwrap();

        let test_keys = vec!["spider_man1", "iron_man1", "captain_america1"];

        for key in test_keys {
            println!("\n=== Searching for non-existent key: '{}' ===", key);
            match reader.read(key.as_bytes()) {
                Ok(SearchResult::Found(value, (ss_tables, levels))) => {
                    println!(" Found key '{}' (unexpected!)", key);
                    println!("  Value size: {} bytes", value.len());
                    println!("  SS-tables searched: {}", ss_tables);
                    println!("  Levels searched: {}", levels);
                }
                Ok(SearchResult::Missing((ss_tables, levels))) => {
                    println!(" Key '{}' not found (expected)", key);
                    println!("  SS-tables searched: {}", ss_tables);
                    println!("  Levels searched: {}", levels);
                }
                Err(e) => {
                    println!(" Error searching for '{}': {}", key, e);
                }
            }
        }
    }
}
