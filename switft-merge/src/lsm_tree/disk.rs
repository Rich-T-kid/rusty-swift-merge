use std::error::Error;
use std::fmt::Display;
use std::fs;
use std::io;
use std::io::Read;
use std::io::Seek;

use crate::memtable::mem::TableEntry;
pub const HEADER_CRC: &str = "054a62a514e1d7d93b2955772fe6070d03a9d58f34a42d85918ac975488dbbe4";
const BLOOM_FILTER_SIZE: usize = 1000;
pub const PAGE_PER_BLOCK: usize = 4;

#[derive(Debug)]
pub enum LsmTreeError {
    IOErr(io::Error),
    UnknownErr(Box<dyn std::error::Error>),
}
impl Display for LsmTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LsmTreeError::IOErr(err) => write!(f, "IO error: {}", err),
            LsmTreeError::UnknownErr(err) => write!(f, "Unknown error: {}", err),
        }
    }
}
impl Error for LsmTreeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LsmTreeError::IOErr(err) => Some(err),
            LsmTreeError::UnknownErr(err) => Some(err.as_ref()),
        }
    }
}

pub struct LsmTreeManager {}
// lsm-reader should return
// # of ss-tables it read
// # of lsm-tree levels it traversed
// self.metrics.ss_table_reads += lsm_reader()
// just log out how many ss_tables this request took, caller doesnt care

impl LsmTreeManager {
    // works within /data directory
    // read only
    pub fn new() -> Result<Self, LsmTreeError> {
        Ok(Self {})
    }
    // ! this is mostly for debugging. should check header is correct, skip bloom filter bool array for now, print out the sparse index for now (index,key)
}

/*
guideline on how to do it below.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

let mut file = File::open("sstable.data")?;

 jump directly to offset
file.seek(SeekFrom::Start(offset))?;

let mut buf = vec![0; block_size];
file.read_exact(&mut buf)?;
*/
type TableResult<T> = Result<T, io::Error>;
struct TableReader {
    f: fs::File,
    data_section_offset: Option<usize>,
    sparse_index_cache: Option<Vec<(u64, Vec<u8>)>>,
}
impl TableReader {
    const HEADER_SIZE: usize = 64; // 64 bytes
    const BLOOM_FILTER_SIZE: usize = BLOOM_FILTER_SIZE;

    fn new(f: fs::File) -> TableResult<Self> {
        Ok(Self {
            f: f,
            data_section_offset: None,
            sparse_index_cache: None,
        })
    }

    fn read_header(&mut self) -> TableResult<Vec<u8>> {
        self.f.seek(io::SeekFrom::Start(0))?;

        let mut buffer = [0u8; Self::HEADER_SIZE];
        self.f.read_exact(&mut buffer)?;
        Ok(buffer.to_vec())
    }

    fn get_bloom_filter(&mut self) -> TableResult<Vec<u8>> {
        self.f.seek(io::SeekFrom::Start(Self::HEADER_SIZE as u64))?;

        let mut buffer = vec![0u8; Self::BLOOM_FILTER_SIZE];
        self.f.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    fn get_sparse_index(&mut self) -> TableResult<Vec<(u64, Vec<u8>)>> {
        // Return cached sparse index if available
        if let Some(ref cached) = self.sparse_index_cache {
            return Ok(cached.clone());
        }

        // Seek to start of sparse index (after header + bloom filter)
        let sparse_index_start = (Self::HEADER_SIZE + Self::BLOOM_FILTER_SIZE) as u64;
        self.f.seek(io::SeekFrom::Start(sparse_index_start))?;

        // Read sparse index size (u32)
        let mut size_buf = [0u8; 4];
        self.f.read_exact(&mut size_buf)?;
        let sparse_index_content_size = u32::from_le_bytes(size_buf) as usize;

        // Calculate and cache data section offset
        // data_section = sparse_index_start + size_field(4) + sparse_index_content_size
        self.data_section_offset =
            Some(sparse_index_start as usize + 4 + sparse_index_content_size);

        // Read sparse index content
        let mut sparse_index_buffer = vec![0u8; sparse_index_content_size];
        self.f.read_exact(&mut sparse_index_buffer)?;

        // Parse sparse index entries: key_len(u32) | key | offset(u64)
        let mut result = Vec::new();
        let mut pos = 0;

        while pos < sparse_index_buffer.len() {
            // Read key length (4 bytes)
            let key_len = u32::from_le_bytes(
                sparse_index_buffer[pos..pos + 4]
                    .try_into()
                    .expect("slice length mismatch"),
            ) as usize;
            pos += 4;

            // Read key
            let key = sparse_index_buffer[pos..pos + key_len].to_vec();
            pos += key_len;

            // Read offset (8 bytes)
            let offset = u64::from_le_bytes(
                sparse_index_buffer[pos..pos + 8]
                    .try_into()
                    .expect("slice length mismatch"),
            );
            pos += 8;

            result.push((offset, key));
        }

        // Cache the sparse index
        self.sparse_index_cache = Some(result.clone());

        Ok(result)
    }
    /*
       search -> Ok<Some(vec<u8>)> | resulting value of the key  ! this could be a tomstone, caller needs to proccess this and check
       search -> Ok<None> | corresponding value does not exist in this ss-table
       search -> Err(x) | IO error occured
    */

    fn search(&mut self, key: &[u8]) -> TableResult<Option<Vec<u8>>> {
        // Get sparse index (will use cache if available)
        let sparse_index = self.get_sparse_index()?;

        // Get data section offset (cached from get_sparse_index)
        let data_offset = self
            .data_section_offset
            .expect("data_section_offset should be set after get_sparse_index");

        // Binary search to find which block contains the key
        let block_idx =
            sparse_index.binary_search_by(|(_, block_key)| block_key.as_slice().cmp(key));

        // Determine which block to search
        let search_offset = match block_idx {
            Ok(idx) => sparse_index[idx].0, // Exact match, start at this block
            Err(0) => return Ok(None), // Key is before first block, since first key is always written in sorted order this key cannot exist in this ss-table
            Err(idx) => sparse_index[idx - 1].0, // Key is in previous block
        };

        // Calculate absolute file position
        let file_position = data_offset as u64 + search_offset;
        self.f.seek(io::SeekFrom::Start(file_position))?;

        // Search through entries until we find the key or pass it
        loop {
            // Read key length (4 bytes)
            let mut key_len_buf = [0u8; 4];
            if self.f.read_exact(&mut key_len_buf).is_err() {
                // End of file or block
                return Ok(None);
            }
            let entry_key_len = u32::from_le_bytes(key_len_buf) as usize;

            // Read key
            let mut entry_key = vec![0u8; entry_key_len];
            self.f.read_exact(&mut entry_key)?;

            // Read value length (4 bytes)
            let mut value_len_buf = [0u8; 4];
            self.f.read_exact(&mut value_len_buf)?;
            let value_len = u32::from_le_bytes(value_len_buf) as usize;

            // Read value
            let mut value = vec![0u8; value_len];
            self.f.read_exact(&mut value)?;

            // Compare keys
            match entry_key.as_slice().cmp(key) {
                std::cmp::Ordering::Equal => return Ok(Some(value)),
                std::cmp::Ordering::Greater => return Ok(None), // Passed the key
                std::cmp::Ordering::Less => continue,           // Keep searching
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
    ) -> [u8; BLOOM_FILTER_SIZE] {
        // the size needs to be very strict should be BLOOM_FILTER_SIZE
        [0u8; 1000]
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
