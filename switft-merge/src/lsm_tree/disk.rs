use std::error::Error;
use std::fmt::Display;
use std::io;

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
}

pub struct BloomGenerator {}
impl BloomGenerator {
    pub fn generate_filter(
        _data: &std::collections::BTreeMap<Vec<u8>, Option<TableEntry>>,
    ) -> [u8; BLOOM_FILTER_SIZE] {
        // the size needs to be very strict should be BLOOM_FILTER_SIZE
        [0u8; 1000]
    }
}
