use crate::lsm_tree::disk;

use super::lsm;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::thread::available_parallelism;
pub const WRITE_AHEAD_LOG_FILE_NAME: &str = "Wal.tmp";
pub const TOMB_STONE_BYTE_REPRESENTATION: u8 = 255;
pub const META_DATA_MAP_DOESNT_EXIST: u8 = 0u8;
pub const META_DATA_MAP_EXIST: u8 = 1u8;
#[derive(Debug, Clone, PartialEq)]
pub enum TrueTypes {
    Unspecified,
    Bool,
    RawBytes,
    String,
    Uint32,
    Uint64,
    Int32,
    Int64,
    Float32,
    Double,
}
impl TrueTypes {
    pub fn enum_variant_value(&self) -> u8 {
        match self {
            TrueTypes::Unspecified => 0u8,
            TrueTypes::Bool => 1u8,
            TrueTypes::RawBytes => 2u8,
            TrueTypes::String => 3u8,
            TrueTypes::Uint32 => 4u8,
            TrueTypes::Uint64 => 5u8,
            TrueTypes::Int32 => 6u8,
            TrueTypes::Int64 => 7u8,
            TrueTypes::Float32 => 8u8,
            TrueTypes::Double => 9u8,
        }
    }
    pub fn to_enum_varient(value: &[u8]) -> Self {
        match value {
            &[1u8] => Self::Bool,
            &[2u8] => Self::RawBytes,
            &[3u8] => Self::String,
            &[4u8] => Self::Uint32,
            &[5u8] => Self::Uint64,
            &[6u8] => Self::Int32,
            &[7u8] => Self::Int64,
            &[8u8] => Self::Float32,
            &[9u8] => Self::Double,
            _ => Self::Unspecified,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct TypeInfoMetadata {
    pub raw: Vec<u8>,
    pub true_type: TrueTypes,
}
impl TypeInfoMetadata {
    pub fn new(raw: Vec<u8>, true_type: TrueTypes) -> Self {
        TypeInfoMetadata { raw, true_type }
    }
}
#[derive(Debug, PartialEq)]
pub struct TableEntry {
    pub value: Vec<u8>,
    pub meta_data: Option<BTreeMap<String, TypeInfoMetadata>>,
}
#[derive(Debug)]
pub enum DecodingError {
    IoError(io::Error),
    MalformedData(String),
    Empty(),
}
impl From<DecodingError> for Box<dyn std::error::Error> {
    fn from(value: DecodingError) -> Self {
        match value {
            DecodingError::IoError(io) => Box::new(io),
            DecodingError::MalformedData(msg) => {
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
            }
            DecodingError::Empty() => Box::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no data to decode",
            )),
        }
    }
}
impl From<DecodingError> for MemtableError {
    fn from(value: DecodingError) -> Self {
        match value {
            DecodingError::IoError(err) => MemtableError::WriteAheadLog(WalError::IoErr(err)),
            DecodingError::Empty() => MemtableError::WriteAheadLog(WalError::EmptyWal()),
            DecodingError::MalformedData(info) => {
                MemtableError::WriteAheadLog(WalError::InvalidStructure(info))
            }
        }
    }
}
impl TableEntry {
    pub fn new(value: Vec<u8>, meta_data: Option<BTreeMap<String, TypeInfoMetadata>>) -> Self {
        TableEntry { value, meta_data }
    }
    pub fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        // val-len | val |
        let mut buffer = Vec::new();
        let val_len = self.value.len() as u32;
        buffer.write_all(&val_len.to_le_bytes())?;
        buffer.write_all(&self.value)?;
        if let Some(md) = &self.meta_data {
            buffer.write_all(&META_DATA_MAP_EXIST.to_le_bytes())?;
            for (key, type_info_md) in md {
                //
                // str-len | str | raw_len | raw | enum_variant as u8
                let k_len = key.len();
                let tp_raw_len = type_info_md.raw.len() as u32;
                buffer.write_all(&(k_len as u32).to_le_bytes())?;
                buffer.write_all(key.as_bytes())?;
                buffer.write_all(&(tp_raw_len as u32).to_le_bytes())?;
                buffer.write_all(&type_info_md.raw)?;
                buffer.write_all(&[type_info_md.true_type.enum_variant_value()])?;
            }
            return Ok(buffer);
        }
        buffer.write_all(&META_DATA_MAP_DOESNT_EXIST.to_le_bytes())?;
        Ok(buffer)
    }
    // takes in a value portion of k-len | k | v-len | v
    // need to parse out the str-len | str | raw-len | raw | enum pairs into table entry
    pub fn deserialize(disk_bytes: Vec<u8>) -> Result<Self, DecodingError> {
        let malformed_error =
            |msg: &str| -> DecodingError { DecodingError::MalformedData(String::from(msg)) };
        let buffer_size = disk_bytes.len();
        const LEN_SIZE: usize = 4;
        if buffer_size <= LEN_SIZE {
            return Err(malformed_error(
                "value buffer does not contain a length prefix for decoding",
            ));
        }
        let mut idx = 0;
        let val_len =
            u32::from_le_bytes(disk_bytes[idx..idx + LEN_SIZE].try_into().unwrap()) as usize; // ! replace with ? for error handleing
        idx += LEN_SIZE;
        if idx + val_len > buffer_size {
            return Err(malformed_error(
                "value buffer does not contain enough bytes for value decoding",
            ));
        }
        let value_bytes = disk_bytes[idx..idx + val_len].to_vec();
        idx += val_len;
        // does metaData map exist?
        if idx + 1 > buffer_size {
            return Err(malformed_error(
                "value buffer does not contain meta data flag",
            ));
        }
        let meta_data_exist = &disk_bytes[idx..idx + 1];
        if meta_data_exist == &[META_DATA_MAP_DOESNT_EXIST] {
            return Ok(TableEntry {
                value: value_bytes,
                meta_data: None,
            });
        } else {
            idx += 1; // move past meta_data_map_exist byte
            let mut meta_data_hashmap: BTreeMap<String, TypeInfoMetadata> = BTreeMap::new();
            loop {
                if idx >= buffer_size {
                    break;
                }
                if idx + LEN_SIZE > buffer_size {
                    return Err(malformed_error(
                        "meta data buffer does not contain enough bytes for key-len prefix",
                    ));
                }
                let key_len =
                    u32::from_le_bytes(disk_bytes[idx..idx + LEN_SIZE].try_into().unwrap())
                        as usize;
                idx += LEN_SIZE;
                if idx + key_len > buffer_size {
                    return Err(malformed_error(
                        "meta data buffer does not contain enough bytes to decode key bytes",
                    ));
                }
                let string_val = &disk_bytes[idx..idx + key_len];
                idx += key_len;
                if idx + LEN_SIZE > buffer_size {
                    return Err(malformed_error(
                        "meta data buffer does not contain enough bytes for byte-len prefix",
                    ));
                }
                let raw_bytes_len =
                    u32::from_le_bytes(disk_bytes[idx..idx + LEN_SIZE].try_into().unwrap())
                        as usize;
                idx += LEN_SIZE;
                if idx + raw_bytes_len > buffer_size {
                    return Err(malformed_error(
                        "meta data buffer does not contain enough bytes to decode raw bytes",
                    ));
                }
                let raw_bytes = &disk_bytes[idx..idx + raw_bytes_len];
                idx += raw_bytes_len;
                if idx + 1 > buffer_size {
                    return Err(malformed_error(
                        "meta data buffer does not contain enough bytes for true type byte",
                    ));
                }
                let true_type = &disk_bytes[idx..idx + 1];
                idx += 1;
                let type_info = TypeInfoMetadata::new(
                    raw_bytes.to_vec(),
                    TrueTypes::to_enum_varient(true_type),
                );
                let string_repr = String::from_utf8(string_val.to_vec()).unwrap();
                meta_data_hashmap.insert(string_repr, type_info);
            }
            return Ok(TableEntry {
                value: value_bytes,
                meta_data: Some(meta_data_hashmap),
            });
        }
    }
}
#[derive(Debug)]
pub struct Memtable {
    pub wal: WalManager,
    in_memory_repr: BTreeMap<Vec<u8>, Option<TableEntry>>,
    config: Option<ConfigInfo>,
    memory_metrics: Arc<Mutex<MemMetricTracker>>,
    disk_metrics: Arc<Mutex<DiskTreeMetricTracker>>,
}

pub struct TransitiveRepr {}
impl TransitiveRepr {
    pub fn new() -> Self {
        TransitiveRepr {}
    }
    /*
    format of entry is key-length | key | value-len | value
    in the case of a tombstone its treated as a regular value but when read into memory its checked against the TOMB_STONE_U32_REPRESENTATION constant. if the value matches its interpreted as a tombstone
    this has a very low likely hood that a value is also this enum but thats a risk we will allow.
    if this value does not match the constant its proccesed as a table_entry
     */
    pub fn to_wal_entry<'a>(
        &self,
        buffer: &mut Vec<u8>,
        key: &'a [u8],
        value: WalEntry,
    ) -> io::Result<()> {
        let key_len = key.len() as u32;
        buffer.write_all(&key_len.to_le_bytes())?;
        buffer.write_all(key)?;
        match value {
            WalEntry::Tombstone() => {
                buffer.write_all(&1u32.to_le_bytes())?;
                buffer.write_all(&TOMB_STONE_BYTE_REPRESENTATION.to_le_bytes())?;
                return Ok(());
                // simple write path,     key-len | key | 4 |tombstone marker (0)
            }
            WalEntry::Value(table_entry) => {
                let contents = table_entry.serialize()?;
                let value_size = contents.len() as u32;
                buffer.write_all(&value_size.to_le_bytes())?;
                buffer.write_all(&contents)?;
                return Ok(());
                // key-len | key | {value-len} | value
            }
        }
    }
}
#[derive(Debug)]
#[allow(dead_code)]
pub enum MemtableError {
    InitError(MemInitError),
    WriteAheadLog(WalError),
    LsmTreeError(lsm::LsmTreeError),
}
impl From<WalError> for MemtableError {
    fn from(value: WalError) -> Self {
        Self::WriteAheadLog(value)
    }
}
impl From<std::io::Error> for MemtableError {
    fn from(value: std::io::Error) -> Self {
        MemtableError::WriteAheadLog(WalError::IoErr(value))
    }
}
impl From<lsm::LsmTreeError> for MemtableError {
    fn from(value: lsm::LsmTreeError) -> Self {
        Self::LsmTreeError(value)
    }
}
impl From<serde_json::Error> for MemtableError {
    fn from(value: serde_json::Error) -> Self {
        let err_msg = value.to_string();

        // Check if it's a missing field error
        if err_msg.contains("missing field") {
            // Extract field name from error message (between backticks)
            if let Some(start) = err_msg.find("`") {
                if let Some(end) = err_msg[start + 1..].find("`") {
                    let field_name = &err_msg[start + 1..start + 1 + end];
                    return MemtableError::InitError(MemInitError::MissingKey(
                        field_name.to_string(),
                    ));
                }
            }
        }

        MemtableError::InitError(MemInitError::InvalidFormat(format!(
            "failed to parse json file: {:?} ",
            value,
        )))
    }
}
impl From<MemInitError> for MemtableError {
    fn from(value: MemInitError) -> Self {
        MemtableError::InitError(value)
    }
}

#[derive(Debug)]
pub enum MemInitError {
    MissingKey(String),
    InvalidArgument(String),
    MissingConfig(),
    InvalidFormat(String),
}

pub enum WalEntry<'a> {
    Value(&'a TableEntry),
    Tombstone(),
}
pub enum ConfigSource<'a> {
    FileSource(&'a String),
    RawBytes(Vec<u8>),
}
use std::sync::{Arc, Mutex};
use std::thread;
fn periodic_metric_flush(metrics: Vec<Arc<Mutex<dyn MetricTracker>>>) {
    loop {
        for m in &metrics {
            match m.lock().unwrap().flush() {
                Ok(_) => {
                    println!(
                        "wrote metrics to file just fine! {:?}",
                        time::Instant::now()
                    )
                }
                Err(e) => {
                    println!("Error writing metrics out, {:?}", e)
                }
            }
        }
        thread::sleep(time::Duration::new(10, 0));
    }
}
impl Memtable {
    pub fn new() -> Result<Self, MemtableError> {
        let mut wal_result = WalManager::new(WRITE_AHEAD_LOG_FILE_NAME)?;
        let wal_contents = wal_result.drain()?;
        let memory_tracker = Arc::new(Mutex::new(MemMetricTracker::new()?));
        let disk_tracker = Arc::new(Mutex::new(DiskTreeMetricTracker::new()?));
        let mut mem = Memtable {
            wal: wal_result,
            in_memory_repr: BTreeMap::new(),
            config: None,
            memory_metrics: Arc::clone(&memory_tracker),
            disk_metrics: Arc::clone(&disk_tracker),
        };
        mem.rebuild_memtable(wal_contents)?;
        let memory_tracker_clone = Arc::clone(&memory_tracker);
        let disk_tracker_clone = Arc::clone(&disk_tracker);

        thread::spawn(move || {
            periodic_metric_flush(vec![
                memory_tracker_clone as Arc<Mutex<dyn MetricTracker>>,
                disk_tracker_clone as Arc<Mutex<dyn MetricTracker>>,
            ]);
            println!("exited multi_thread_fn")
        });

        Ok(mem)
    }
    // mainly just for testing
    #[allow(dead_code)]
    fn with_wal_manager(wal_manager: WalManager) -> Self {
        Memtable {
            wal: wal_manager,
            in_memory_repr: BTreeMap::new(),
            config: None,
            memory_metrics: Arc::new(Mutex::new(MemMetricTracker::new().unwrap())),
            disk_metrics: Arc::new(Mutex::new(DiskTreeMetricTracker::new().unwrap())),
        }
    }

    pub fn put(&mut self, key: &[u8], value: TableEntry) -> Result<(), MemtableError> {
        if self.should_flush() {
            self.flush()?;
            self.memory_metrics.lock().unwrap().flush_counter += 1;
        }
        self.memory_metrics.lock().unwrap().memtable_writes += 1;
        let mut wal_entry_repr = Vec::new();
        TransitiveRepr::new().to_wal_entry(&mut wal_entry_repr, key, WalEntry::Value(&value))?;

        self.wal.write_entry(wal_entry_repr.as_slice())?;
        self.in_memory_repr.insert(key.to_vec(), Some(value));
        Result::Ok(())
    }
    pub fn delete(&mut self, key: &[u8]) -> Result<(), MemtableError> {
        if self.should_flush() {
            self.flush()?;
            self.memory_metrics.lock().unwrap().flush_counter += 1;
        }
        self.memory_metrics.lock().unwrap().memtable_writes += 1;
        let mut wal_entry_repr = Vec::new();
        TransitiveRepr::new().to_wal_entry(&mut wal_entry_repr, key, WalEntry::Tombstone())?;
        self.wal.write_entry(wal_entry_repr.as_slice())?;
        self.in_memory_repr.insert(key.to_vec(), None);
        Result::Ok(())
    }
    pub fn get(&mut self, key: &[u8]) -> Result<&Option<TableEntry>, lsm::LsmTreeError> {
        self.memory_metrics.lock().unwrap().memtable_reads += 1;
        match self.in_memory_repr.get(key) {
            None => {
                return {
                    self.memory_metrics.lock().unwrap().lsm_reads += 1;
                    // lsm-reader should return
                    // # of ss-tables it read
                    // # of lsm-tree levels it traversed
                    // self.metrics.ss_table_reads += lsm_reader()
                    // just log out how many ss_tables this request took, caller doesnt care
                    Err(lsm::LsmTreeError::Unimplemented())
                };
            } // ! if it doesnt exist in memory, read from disk (tbd)
            Some(value) => Ok(value),
        }
    }
    // write in memory contents out to lsm tree as Level 0
    fn flush(&mut self) -> Result<(), lsm::LsmTreeError> {
        let start = time::Instant::now();

        /*



        */

        self.memory_metrics
            .lock()
            .unwrap()
            .flush_duration
            .push(start.elapsed());
        Result::Ok(())
    }
    // read from WAL and reconstruct memtable
    // ! todo: might needs to place some locking on the file or something?
    pub fn rebuild_memtable(&mut self, wal_content: Vec<u8>) -> Result<(), MemtableError> {
        let too_small_parsing_err = |msg: &str| -> Result<(), MemtableError> {
            Err(MemtableError::WriteAheadLog(WalError::InvalidStructure(
                String::from(msg),
            )))
        };
        const LEN_SIZE: usize = 4;
        let max_size = wal_content.len();
        if max_size == 0 {
            // ** first time constructing memtable
            return Ok(());
        }
        if max_size < LEN_SIZE {
            // if no bytes or atleast not enough bytes for the key-len read then return err early
            return Err(MemtableError::WriteAheadLog(WalError::EmptyWal()));
        }
        let mut curr_idx = 0;
        // at each step where we access the buffer there needs to be bounds checks,
        // if the bounds check fail then a memtable::Wal::malformed should be returned
        loop {
            // wal structure is key-len (u32) | key | value-len (u32) | value
            //                                            ^
            if curr_idx >= max_size {
                break;
                //return Err(MemtableError::WriteAheadLog(WalError::InvalidStructure()));
            }
            let key_len = u32::from_le_bytes(
                wal_content[curr_idx..curr_idx + LEN_SIZE]
                    .try_into()
                    .unwrap(),
            ) as usize;
            if key_len == 0 {
                return too_small_parsing_err("key-length prefix cannot be 0");
            }
            curr_idx += LEN_SIZE;
            if curr_idx + key_len >= max_size {
                return too_small_parsing_err(
                    "key-entry contains a length prefix that is larger than the buffer",
                );
            }
            let key_value = wal_content[curr_idx..curr_idx + key_len].to_vec();
            curr_idx += key_len;
            if curr_idx + LEN_SIZE >= max_size {
                return too_small_parsing_err("value-len prefix is not present in the buffer");
            }
            let val_len = u32::from_le_bytes(
                wal_content[curr_idx..curr_idx + LEN_SIZE]
                    .try_into()
                    .unwrap(),
            ) as usize;
            // to parse the value there are two cases, tombstone and non tombstone, we will grab the value from
            // value-len like normal but we will compare the returned bytes to the constant TOMB_STONE_BYTE_REPRESENTATION
            // if they match then we have the entire entry and we can add this into the memtable
            curr_idx += LEN_SIZE;
            if curr_idx + val_len > max_size {
                return too_small_parsing_err(
                    "value-entry contains a length prefix that is larger than the buffer",
                );
            }
            let raw_value = wal_content[curr_idx..curr_idx + val_len].to_vec();
            curr_idx += val_len;
            let table_entry = {
                if raw_value == [TOMB_STONE_BYTE_REPRESENTATION] {
                    // this is a tombstone entry so now we can write into mem table with value set to None
                    (key_value, None)
                } else {
                    let x = TableEntry::deserialize(raw_value)?;
                    (key_value, Some(x))
                }
            };
            self.in_memory_repr.insert(table_entry.0, table_entry.1);
        }
        Result::Ok(())
    }

    // need to read from config to handle this
    // checks if conditions are met to flush
    fn should_flush(&self) -> bool {
        false
    }
    // should be able to handle a file or a load of bytes (grpc call)
    pub fn update_config(&mut self, content: ConfigSource) -> Result<(), MemtableError> {
        match content {
            ConfigSource::FileSource(name) => {
                let config_as_str = fs::read_to_string(name)
                    .map_err(|_| MemtableError::InitError(MemInitError::MissingConfig()))?;
                if config_as_str.len() == 0 {
                    return Err(MemtableError::InitError(MemInitError::MissingConfig()));
                }
                let mut config: ConfigInfo = serde_json::from_str(&config_as_str)?;
                config.validate()?;
                self.config = Some(config);
            }
            _ => {
                todo!()
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ConfigInfo {
    ram_max_size: u32,
    ram_max_time: u16,
    target_chunks: u8,
    compaction_check_interval_seconds: u16,
    wal_enabled: bool,
    bloom_false_positive_rate: f64,
    max_compaction_threads: u8,
}
impl ConfigInfo {
    const KILOBYTE: u32 = 1024;
    const MEGABYTE: u32 = Self::KILOBYTE * Self::KILOBYTE;
    fn validate(&mut self) -> Result<(), MemInitError> {
        // go through each field, follow config.md guidelines
        if self.ram_max_size < Self::KILOBYTE || self.ram_max_size > Self::MEGABYTE * 2048 {
            return Err(MemInitError::InvalidArgument(format!(
                "key:(ram_max_size) has value outside valid range (1KB,2GB)"
            )));
        }

        if self.ram_max_time < 10 || self.ram_max_time > 10080 {
            return Err(MemInitError::InvalidArgument(format!(
                "key:(ram_max_time) has value outside valid range (10,10080) [10 seconds, 168 hours]"
            )));
        }

        if self.target_chunks < 2 || self.target_chunks > 128 {
            return Err(MemInitError::InvalidArgument(format!(
                "key:(target_chunks) has value outside valid range (2,128)"
            )));
        }

        if self.compaction_check_interval_seconds < 1
            || self.compaction_check_interval_seconds > (60 * 60) * 4
        {
            // [1 second ,4 hours]
            return Err(MemInitError::InvalidArgument(format!(
                "key:(compaction_check_interval_seconds) has value outside valid range (1,14400) [1 second, 4 hours]"
            )));
        }

        if self.bloom_false_positive_rate < 0.001 || self.bloom_false_positive_rate > 0.1 {
            return Err(MemInitError::InvalidArgument(format!(
                "key:(bloom_false_positive_rate) has value outside valid range (0.001,0.1) [0.1% to 10%]"
            )));
        }

        if self.max_compaction_threads < 1 {
            return Err(MemInitError::InvalidArgument(format!(
                "key:(max_compaction_threads) has value outside valid range (1,system_thread_max)"
            )));
        }
        self.max_compaction_threads = std::cmp::min(
            self.max_compaction_threads,
            available_parallelism().unwrap().get() as u8,
        );

        Ok(())
    }
}
trait MetricTracker {
    fn flush(&self) -> Result<(), MemtableError>;
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub(crate) struct MemMetricTracker {
    pub(crate) memtable_reads: u64,
    pub(crate) lsm_reads: u64,
    pub(crate) ss_table_reads: u64, //(for global data, will also be a local one for GET request)
    // writes
    pub(crate) memtable_writes: u64,
    pub(crate) flush_counter: u64,
    //auxiliary
    pub(crate) flush_duration: Vec<time::Duration>,
}
// lsm-tree (ss-tables)

#[derive(Serialize, Deserialize, Debug, Default)]
pub(crate) struct DiskTreeMetricTracker {
    pub(crate) total_ss_tables_merged: u64,
    pub(crate) merge_output_size: Vec<u64>,
}
impl MemMetricTracker {
    const FILEPATH: &str = "tmp/memory_metrics.json";
    pub(crate) fn new() -> Result<Self, MemtableError> {
        let content = match fs::read(Self::FILEPATH) {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e.into()),
        };
        if content.is_empty() {
            return Ok(Self::default());
        }
        let tracker: MemMetricTracker = serde_json::from_slice(&content)?;
        Ok(tracker)
    }

    pub(crate) fn flush_metrics(&self) -> Result<(), MemtableError> {
        let json_data = serde_json::to_string_pretty(self)?;
        if let Some(parent) = std::path::Path::new(Self::FILEPATH).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(Self::FILEPATH, json_data)?;
        Ok(())
    }

    pub(crate) fn new_with_file_path(filepath: &str) -> Result<Self, MemtableError> {
        let content = match fs::read(filepath) {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e.into()),
        };
        if content.is_empty() {
            return Ok(Self::default());
        }
        let tracker: MemMetricTracker = serde_json::from_slice(&content)?;
        Ok(tracker)
    }

    pub(crate) fn flush_metrics_with_fp(&self, filepath: &str) -> Result<(), MemtableError> {
        let json_data = serde_json::to_string_pretty(self)?;
        if let Some(parent) = std::path::Path::new(filepath).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(filepath, json_data)?;
        Ok(())
    }
}
impl MetricTracker for MemMetricTracker {
    fn flush(&self) -> Result<(), MemtableError> {
        self.flush_metrics()
    }
}
impl DiskTreeMetricTracker {
    const FILEPATH: &str = "tmp/disk_metrics.json";
    fn new() -> Result<Self, MemtableError> {
        let content = match fs::read(Self::FILEPATH) {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e.into()),
        };
        if content.is_empty() {
            return Ok(Self::default());
        }
        let tracker: DiskTreeMetricTracker = serde_json::from_slice(&content)?;
        Ok(tracker)
    }

    fn flush_metrics(&self) -> Result<(), MemtableError> {
        let json_data = serde_json::to_string_pretty(self)?;
        if let Some(parent) = std::path::Path::new(Self::FILEPATH).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(Self::FILEPATH, json_data)?;
        Ok(())
    }

    pub(crate) fn new_with_file_path(filepath: &str) -> Result<Self, MemtableError> {
        let content = match fs::read(filepath) {
            Ok(data) => data,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(e.into()),
        };
        if content.is_empty() {
            return Ok(Self::default());
        }
        let tracker: DiskTreeMetricTracker = serde_json::from_slice(&content)?;
        Ok(tracker)
    }

    pub(crate) fn flush_metrics_with_fp(&self, filepath: &str) -> Result<(), MemtableError> {
        let json_data = serde_json::to_string_pretty(self)?;
        if let Some(parent) = std::path::Path::new(filepath).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(filepath, json_data)?;
        Ok(())
    }
}
impl MetricTracker for DiskTreeMetricTracker {
    fn flush(&self) -> Result<(), MemtableError> {
        self.flush_metrics()
    }
}
#[derive(Debug)]
#[allow(dead_code)]
pub struct WalManager {
    f: std::fs::File,
    file_name: String,
}
#[derive(Debug)]
#[allow(dead_code)]
pub enum WalError {
    IoErr(std::io::Error),
    InvalidStructure(String),
    EmptyWal(),
}
impl From<std::io::Error> for WalError {
    fn from(e: std::io::Error) -> Self {
        WalError::IoErr(e)
    }
}
use std::io::{self, ErrorKind, Read, Seek, Write};
use std::{fs, time};
#[allow(dead_code)]
impl WalManager {
    pub fn new(file_name: &str) -> Result<Self, WalError> {
        let f = match std::fs::File::options()
            .read(true)
            .write(true)
            .append(true)
            .create(true)
            .open(file_name)
        {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let result = std::fs::File::create(file_name);
                match result {
                    Ok(new_file) => new_file,
                    Err(e) => return Err(WalError::IoErr(e)),
                }
            }
            Err(e) => return Err(WalError::IoErr(e)),
        };
        Result::Ok(WalManager {
            f: f,
            file_name: file_name.to_string(),
        })
    }
    // should callers pre-serialize the content they want written?
    // seperate the produces from consumers
    pub fn write_entry(&mut self, entry: &[u8]) -> Result<(), WalError> {
        self.f.seek(std::io::SeekFrom::End(0)).unwrap();
        match self.f.write(entry) {
            Ok(_) => Ok(()),
            Err(err) => Err(WalError::IoErr(err)),
        }
    }
    // ! for now we'll stick to using drain as the main way to consume the WAL and rebuild the
    // ! memtable, the issue is that this could cause memory strain, as we write all the files contents
    // ! to a buffer then while this memory buffer exist we iterate across it to build our in memory structs
    // ! before releasing the memory, so just before the memtable is fully constructed there is (WAL_memory_size * 2) bytes of ram being used
    // ! this could play a role later when we decided how we want to divy up system resources for different task.
    // ! also reading/reconstructing the memtable from the WAL should be extremely rare so again for now its fine.
    // ! possible improvments (read from disk to a medium sized buffer (1-5 MB) and build structs from buffer before refilling from disk, removes the risk of Out Of Memory since at most its the memtable_size + (1-5)MB )
    // consume all the contents of the WAl, this doesnt not delete the current contents of the WAL
    pub fn drain(&mut self) -> Result<Vec<u8>, WalError> {
        self.f.seek(std::io::SeekFrom::Start(0)).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let _ = match self.f.read_to_end(&mut buf) {
            Ok(size) => size,
            Err(err) => return Err(WalError::IoErr(err)),
        };
        Ok(buf)
    }
    // remove all the elements within the WAL
    pub fn clear(&mut self) -> Result<(), WalError> {
        match self.f.set_len(0) {
            Ok(()) => Ok(()),
            Err(err) => Err(WalError::IoErr(err)),
        }
    }
    // call wal_iterator.iter() , this allows for multiple iterators that arent tied to WalManager directory
    pub fn wal_iterator() -> WalIterator {
        WalIterator {}
    }
    fn remove_file(&mut self) -> Result<(), std::io::Error> {
        std::fs::remove_file(self.file_name.clone())
    }
}
pub struct WalIterator {}
// ! returns one entry at a time until EOF
impl Iterator for WalIterator {
    type Item = Vec<u8>;
    fn next(&mut self) -> Option<Self::Item> {
        Some(vec![10u8])
    }
}

#[cfg(test)]
mod wal_manager_test {
    use crate::memtable::mem::WalManager;
    use rand::prelude::*;
    const WAL_MANAGER_TEST_FILE_NAME: &str = "wal_M_test_file";
    fn gen_file_name() -> String {
        let mut int_vec = vec![];
        for _ in 1..3 {
            let rand_val = rand::rng().random_range(100..2000);
            int_vec.push(rand_val);
        }
        let single_interger: Vec<String> = int_vec.iter().map(|i| i.to_string()).collect();
        let single_interger = single_interger.join("");
        format!("{WAL_MANAGER_TEST_FILE_NAME}{}.tmp", single_interger).replace("", "")
    }

    #[test]
    fn test_file_name_construct() {
        let name = gen_file_name();
        println!("generated file name:\t{name}")
    }
    #[test]
    fn test_init() {
        let mut manager = WalManager::new(&gen_file_name()).expect("failed to create WalManager");

        manager.remove_file().expect("failed to remove WAL file");
    }
    #[test]
    fn test_basic_write() {
        let file_name = &gen_file_name();
        let mut manager = WalManager::new(file_name).expect("failed to create Wal Manager");
        manager
            .write_entry("first entry into WAL ==============".as_bytes())
            .expect("failed to write to WAL file");
        manager
            .remove_file()
            .expect(format!("failed to delete {file_name}").as_str())
    }
    #[test]
    fn test_basic_drain() {
        let file_name = &gen_file_name();
        let mut manager = WalManager::new(file_name).expect("failed to create Wal Manager");
        for i in 0..=21 {
            manager
                .write_entry(format!("entry {i}").as_bytes())
                .expect(format!("failed to write {i} into WAL").as_str())
        }
        let content = manager.drain().unwrap();
        let s = std::str::from_utf8(content.as_slice()).unwrap();
        println!("content as a string => {s}");
        manager.remove_file().expect("failed to remove WAL file");
    }
    #[test]
    fn test_basic_clear() {
        let file_name = &gen_file_name();
        let mut manager = WalManager::new(file_name).expect("failed to create Wal Manager");
        for i in 0..=210 {
            manager
                .write_entry(format!("entry {i}").as_bytes())
                .expect(format!("failed to write {i} into WAL").as_str())
        }
        let content = manager.drain().unwrap();
        let og_size = content.len();
        manager.clear().unwrap();
        let new_size = manager.drain().unwrap().len();
        println!("og size: {og_size}");
        println!("new size: {new_size}");
        assert_eq!(new_size, 0);
        manager.remove_file().expect("failed to remove WAL file");
    }
    #[test]
    fn test_basic_intergration_tt() {
        let file_name = &gen_file_name();
        // ! create manager
        let mut manager = WalManager::new(file_name).expect("failed to create Wal Manager");

        // ! write to WAL
        for values in 12u32..42u32 {
            let content = format!("{}:{}", values, values * 3);
            manager.write_entry(content.as_bytes()).unwrap();
        }
        // ! drain the wal
        let wal_contents = manager.drain().unwrap();
        println!("(1) size of wal content : {}", wal_contents.len());
        // ! remove contents of file
        manager.clear().unwrap();

        // ! validate that contenets have been removed
        let wal_contents = manager.drain().unwrap();
        //println!("{string_repr}");
        println!("(2) size of wal content : {}", wal_contents.len());
        assert_eq!(wal_contents.len(), 0);
        // ! test clean up
        manager.remove_file().unwrap();
    }
}

mod config_test {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_valid_config() {
        let config_json = json!({
            "ramMaxSize": 1048576,
            "ramMaxTime": 60,
            "targetChunks": 10,
            "compactionCheckIntervalSeconds": 2,
            "walEnabled": true,
            "bloomFalsePositiveRate": 0.01,
            "maxCompactionThreads": 4
        });

        let mut config: ConfigInfo = serde_json::from_value(config_json).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_ram_max_size_too_small() {
        let config_json = json!({
            "ramMaxSize": 512,
            "ramMaxTime": 60,
            "targetChunks": 10,
            "compactionCheckIntervalSeconds": 2,
            "walEnabled": true,
            "bloomFalsePositiveRate": 0.01,
            "maxCompactionThreads": 4
        });

        let mut config: ConfigInfo = serde_json::from_value(config_json).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(MemInitError::InvalidArgument(msg)) => {
                assert!(msg.contains("ram_max_size"));
            }
            _ => panic!("Expected InvalidArgument error for ram_max_size"),
        }
    }

    #[test]
    fn test_bloom_false_positive_rate_out_of_bounds() {
        let config_json = json!({
            "ramMaxSize": 1048576,
            "ramMaxTime": 60,
            "targetChunks": 10,
            "compactionCheckIntervalSeconds": 2,
            "walEnabled": true,
            "bloomFalsePositiveRate": 0.15,
            "maxCompactionThreads": 4
        });

        let mut config: ConfigInfo = serde_json::from_value(config_json).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(MemInitError::InvalidArgument(msg)) => {
                assert!(msg.contains("bloom_false_positive_rate"));
            }
            _ => panic!("Expected InvalidArgument error for bloom_false_positive_rate"),
        }
    }

    #[test]
    fn test_target_chunks_too_low() {
        let config_json = json!({
            "ramMaxSize": 1048576,
            "ramMaxTime": 60,
            "targetChunks": 1,
            "compactionCheckIntervalSeconds": 2,
            "walEnabled": true,
            "bloomFalsePositiveRate": 0.01,
            "maxCompactionThreads": 4
        });

        let mut config: ConfigInfo = serde_json::from_value(config_json).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(MemInitError::InvalidArgument(msg)) => {
                assert!(msg.contains("target_chunks"));
            }
            _ => panic!("Expected InvalidArgument error for target_chunks"),
        }
    }

    #[test]
    fn test_ram_max_time_out_of_bounds() {
        let config_json = json!({
            "ramMaxSize": 1048576,
            "ramMaxTime": 5,
            "targetChunks": 10,
            "compactionCheckIntervalSeconds": 2,
            "walEnabled": true,
            "bloomFalsePositiveRate": 0.01,
            "maxCompactionThreads": 4
        });

        let mut config: ConfigInfo = serde_json::from_value(config_json).unwrap();
        let result = config.validate();
        assert!(result.is_err());
        match result {
            Err(MemInitError::InvalidArgument(msg)) => {
                assert!(msg.contains("ram_max_time"));
            }
            _ => panic!("Expected InvalidArgument error for ram_max_time"),
        }
    }
}
