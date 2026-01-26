use super::lsm;
use std::collections::{BTreeMap, HashMap};
pub const WRITE_AHEAD_LOG_FILE_NAME: &str = "Wal.tmp";
pub const TOMB_STONE_BYTE_REPRESENTATION: u32 = 0u32;
pub const META_DATA_MAP_DOESNT_EXIST: i32 = -1i32;
pub const META_DATA_MAP_EXIST: i32 = -2i32;
#[derive(Debug, Clone)]
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
}
#[derive(Debug, Clone)]
pub struct TypeInfoMetadata {
    pub raw: Vec<u8>,
    pub true_type: TrueTypes,
}
impl TypeInfoMetadata {
    pub fn new(raw: Vec<u8>, true_type: TrueTypes) -> Self {
        TypeInfoMetadata { raw, true_type }
    }
}
#[derive(Debug)]
pub struct TableEntry {
    pub value: Vec<u8>,
    pub meta_data: Option<HashMap<String, TypeInfoMetadata>>,
}
impl TableEntry {
    pub fn new(value: Vec<u8>, meta_data: Option<HashMap<String, TypeInfoMetadata>>) -> Self {
        TableEntry { value, meta_data }
    }
    pub fn serialize(&self) -> Result<Vec<u8>, io::Error> {
        // val-len | val |
        let mut buffer = Vec::new();
        let val_len = self.value.len() as u32;
        buffer.write_all(val_len.to_le_bytes().as_slice())?;
        buffer.write_all(self.value.as_slice())?;
        if let Some(md) = &self.meta_data {
            buffer.write_all(META_DATA_MAP_EXIST.to_le_bytes().as_slice())?;
            for (key, type_info_md) in md {
                // str-len | str | raw_len | raw | enum_variant as u8
                let k_len = key.len();
                let tp_raw_len = type_info_md.raw.len() as u32;
                buffer.write_all((k_len as u32).to_le_bytes().as_slice())?;
                buffer.write_all(key.as_bytes())?;
                buffer.write_all((tp_raw_len as u32).to_le_bytes().as_slice())?;
                buffer.write_all(&type_info_md.raw)?;
                buffer.write_all(vec![type_info_md.true_type.enum_variant_value()].as_slice())?;
            }
            return Ok(buffer);
        }
        buffer.write_all(META_DATA_MAP_DOESNT_EXIST.to_le_bytes().as_slice())?;
        Ok(buffer)
    }
    fn deserialize(disk_bytes: Vec<u8>) -> Self {
        TableEntry {
            value: vec![],
            meta_data: None,
        }
    }
}
#[derive(Debug)]
pub struct Memtable {
    wal: WalManager,
    in_memory_repr: BTreeMap<Vec<u8>, Option<TableEntry>>,
}

pub struct TransitiveRepr {}
impl TransitiveRepr {
    pub fn new() -> Self {
        TransitiveRepr {}
    }
    pub fn to_wal_entry<'a>(
        &self,
        buffer: &mut Vec<u8>,
        key: &'a [u8],
        value: WalEntry,
    ) -> io::Result<()> {
        let key_len = key.len() as u32;
        buffer.write_all(key_len.to_le_bytes().as_slice())?;
        buffer.write_all(key)?;
        match value {
            WalEntry::Tombstone() => {
                buffer.write_all(4u32.to_le_bytes().as_slice())?;
                buffer.write_all(TOMB_STONE_BYTE_REPRESENTATION.to_le_bytes().as_slice())?;
                return Ok(());
                // simple write path,     key-len | key | 4 |tombstone marker (0)
            }
            WalEntry::Value(table_entry) => {
                let contents = table_entry.serialize().unwrap();
                let value_size = contents.len() as u32;
                buffer.write_all(value_size.to_le_bytes().as_slice())?;
                buffer.write_all(&contents)?;
                return Ok(());
                // key-len | key | {value-len} | value
            }
        }
    }
    fn from_wal_entry<'a>() -> &'a [u8] {
        &[3u8]
    }
}
#[derive(Debug)]
pub enum MemtableError {
    InitError(WalError),
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
/*#[derive(Debug)]
pub enum LsmTreeError {
    Unimplemented(),
    ErrorFlushing(),
}*/
pub enum WalEntry<'a> {
    Value(&'a TableEntry),
    Tombstone(),
}
impl Memtable {
    pub fn new() -> Result<Self, MemtableError> {
        let wal_result = WalManager::new(WRITE_AHEAD_LOG_FILE_NAME);
        if let Err(x) = wal_result {
            return Err(MemtableError::InitError(x));
        };
        Ok(Memtable {
            wal: wal_result.unwrap(),
            in_memory_repr: BTreeMap::new(),
        })
    }
    // mainly just for testing
    fn with_wal_manager(wal_manager: WalManager) -> Self {
        Memtable {
            wal: wal_manager,
            in_memory_repr: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, key: &[u8], value: TableEntry) -> Result<(), MemtableError> {
        if self.should_flush() {
            self.flush()?
        }
        let mut wal_entry_repr = Vec::new();
        TransitiveRepr::new().to_wal_entry(&mut wal_entry_repr, key, WalEntry::Value(&value))?;
        self.wal.write_entry(wal_entry_repr.as_slice())?;
        self.in_memory_repr.insert(key.to_vec(), Some(value));
        Result::Ok(())
    }
    pub fn delete(&mut self, key: &[u8]) -> Result<(), MemtableError> {
        let mut wal_entry_repr = Vec::new();
        TransitiveRepr::new().to_wal_entry(&mut wal_entry_repr, key, WalEntry::Tombstone())?;
        self.wal.write_entry(wal_entry_repr.as_slice())?;
        self.in_memory_repr.insert(key.to_vec(), None);
        Result::Ok(())
    }
    pub fn get(&self, key: &[u8]) -> Result<&Option<TableEntry>, lsm::LsmTreeError> {
        match self.in_memory_repr.get(key) {
            None => return Err(lsm::LsmTreeError::Unimplemented()), // ! if it doesnt exist in memory, read from disk (tbd)
            Some(value) => Ok(value),
        }
    }
    // write in memory contents out to lsm tree as Level 0
    fn flush(&mut self) -> Result<(), lsm::LsmTreeError> {
        Result::Ok(())
    }
    // read from WAL and reconstruct memtable
    fn consume_wal(&self) -> Result<(), MemtableError> {
        Result::Ok(())
    }
    // need to read from config to handle this
    // checks if conditions are met to flush
    fn should_flush(&self) -> bool {
        false
    }
}
// This will be useful when we are flushing to disk
// output elements in sorted order <low key -> high key)
impl Iterator for Memtable {
    type Item = Option<Vec<u8>>;
    fn next(&mut self) -> Option<Self::Item> {
        Some(Some(vec![8u8]))
    }
}

// ! tbd :: type WalManagerResult<T> = std::result::Result<T, WalError>;
#[derive(Debug)]
pub struct WalManager {
    f: std::fs::File,
    file_name: String,
}
#[derive(Debug)]
pub enum WalError {
    IoErr(std::io::Error),
    EmptyWal(),
}
impl From<std::io::Error> for WalError {
    fn from(e: std::io::Error) -> Self {
        WalError::IoErr(e)
    }
}
use std::io::{self, ErrorKind, Read, Seek, Write};
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
    // consume all the contents of the WAl, this doesnt not delete the current contents of the WAL
    pub fn drain(&mut self) -> Result<Vec<u8>, WalError> {
        let fs_size = self.f.metadata().unwrap().len();
        println!("(drain) wal is {fs_size} bytes");
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
        println!("{name}")
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
