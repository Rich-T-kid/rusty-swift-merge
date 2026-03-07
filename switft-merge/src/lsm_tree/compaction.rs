// BASE BRANCH Issue #21
use std::collections::HashMap;
use std::io;

use crate::memtable::mem::ConfigInfo;
#[derive(PartialEq, Eq, Hash)]
pub enum CompactionEvents {
    Init, // for config changes
    CompactonStarted,
    CompactionFinished(u16, u16), // ssTableReader needs to know to update indexes : (input ss-tables,output-sstables)
}
pub enum TableCorruption {
    InvalidCRC(String),    //crc that was there instead of the correct one
    InvalidFooter(String), // footer places in wrong space?
    InvalidSparseIndex(),  // missing sparse index len or missing sparse index
    DataSectionError(),    // what ever other error will be this
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
    update_funcs: HashMap<CompactionEvents, Vec<Box<dyn FnMut()>>>, // pass in functions to call once compaction even occures
    config: ComapctionCofig,
}
impl CompactionCoordinator {
    pub fn new(
        config: &ConfigInfo,
        caller_events: Vec<(CompactionEvents, Box<dyn FnMut()>)>,
    ) -> Self {
        let mut update_funcs: HashMap<CompactionEvents, Vec<Box<dyn FnMut()>>> = HashMap::new();
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
        }
    }
    fn monitor(&mut self) { // look over the /data directory and when certain propertys are met I.E config-file params are met then call size-tier-compaction
    }
    fn update_config(new_config: &ConfigInfo) {} // needs to correspond to memtable::update_config so changes propogate
    async fn size_tier_compaction(&mut self) -> Result<(), CompactionError> {
        // once conditions are met call this function
        Ok(())
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
