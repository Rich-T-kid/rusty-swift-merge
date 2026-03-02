mod lsm_tree;
mod memtable;
mod service;
use crate::{lsm_tree::disk::LsmTreeManager, memtable::mem, service::init_logger};
use std::{
    env,
    sync::{Arc, Mutex, RwLock},
};
const USAGE: &str = "Usage: <./swift-merge> <path/to/config>";
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logger()?;
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        log::error!("failed to pass in lsm-tree config file\n{}", USAGE);
        std::process::exit(1);
    }

    let table = match mem::Memtable::new(mem::ConfigSource::FileSource(&args[1])) {
        Ok(tb) => tb,
        Err(err) => {
            log::error!("failed to generate memtable {:?}", err);
            std::process::exit(1);
        }
    };

    log::info!("Config loaded successfully, starting application...");

    // --- Start gRPC Server ---
    let memtable_instance = Arc::new(RwLock::new(table));
    let lsm_tree = Arc::new(LsmTreeManager::new()?);

    let addr = "0.0.0.0:50051".parse()?;
    service::run_server(memtable_instance, lsm_tree, addr).await?;
    Ok(())
}
