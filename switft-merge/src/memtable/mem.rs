use std::collections::BTreeMap;
const Write_Ahead_Log_File_Name: &str = "Wal.tmp";
const Tombo_Stone_Byte_Representation: u8 = 0u8;
pub struct memtable {
    wal: WalManager,
    in_memory_repr: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}
struct transitive_repr {}
impl transitive_repr {
    fn new() -> Self {
        transitive_repr {}
    }
    fn to_wal_entry<'a, 'b>(&self, key: &'a [u8], value: WalEntry) -> &'a [u8] {
        &[3u8]
    }
    fn from_wal_entry<'a>() -> &'a [u8] {
        &[3u8]
    }
}
#[derive(Debug)]
pub enum memtableError {
    InitError(WalError),
    WriteAheadLog(WalError),
    LsmTreeError(lsmTreeError),
}
impl From<WalError> for memtableError {
    fn from(value: WalError) -> Self {
        Self::WriteAheadLog(value)
    }
}
impl From<lsmTreeError> for memtableError {
    fn from(value: lsmTreeError) -> Self {
        Self::LsmTreeError(value)
    }
}
#[derive(Debug)]
pub enum lsmTreeError {
    unimplemented(),
    ErrorFlushing(),
}
enum WalEntry<'a> {
    Value(&'a [u8]),
    Tombstone(),
}
impl memtable {
    pub fn new() -> Result<Self, memtableError> {
        let wal_result = WalManager::new(Write_Ahead_Log_File_Name);
        if let Err(x) = wal_result {
            return Err(memtableError::InitError(x));
        };
        Ok(memtable {
            wal: wal_result.unwrap(),
            in_memory_repr: BTreeMap::new(),
        })
    }
    // mainly just for testing
    fn with_wal_manager(wal_manager: WalManager) -> Self {
        memtable {
            wal: wal_manager,
            in_memory_repr: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<(), memtableError> {
        if self.should_flush() {
            self.flush()?
        }
        let wal_entry_repr = transitive_repr::new().to_wal_entry(key, WalEntry::Value(value));
        self.wal.write_entry(wal_entry_repr)?;
        self.in_memory_repr
            .insert(key.to_vec(), Some(value.to_vec()));
        Result::Ok(())
    }
    pub fn delete(&mut self, key: &[u8]) -> Result<(), memtableError> {
        let wal_entry_repr = transitive_repr::new().to_wal_entry(key, WalEntry::Tombstone());
        self.wal.write_entry(wal_entry_repr)?;
        self.in_memory_repr.insert(key.to_vec(), None);
        Result::Ok(())
    }
    pub fn get(&self, key: &[u8]) -> Result<&Option<Vec<u8>>, lsmTreeError> {
        match self.in_memory_repr.get(key) {
            None => return Err(lsmTreeError::unimplemented()), // ! if it doesnt exist in memory, read from disk (tbd)
            Some(value) => Ok(value),
        }
    }
    // write in memory contents out to lsm tree as Level 0
    fn flush(&mut self) -> Result<(), lsmTreeError> {
        Result::Ok(())
    }
    // read from WAL and reconstruct memtable
    fn consume_wal(&self) -> Result<(), memtableError> {
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
impl Iterator for memtable {
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
use std::{
    fs::{self, File, read},
    io::{ErrorKind, Read, Seek, Write},
};
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
        let result = match self.f.read_to_end(&mut buf) {
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
    const Wal_manager_test_file_name: &str = "wal_M_test_file";
    fn gen_file_name() -> String {
        let mut int_vec = vec![];
        for _ in 1..3 {
            let rand_val = rand::rng().random_range(100..2000);
            int_vec.push(rand_val);
        }
        let single_interger: Vec<String> = int_vec.iter().map(|i| i.to_string()).collect();
        let single_interger = single_interger.join("");
        format!("{Wal_manager_test_file_name}{}.tmp", single_interger).replace("", "")
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
