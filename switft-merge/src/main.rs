mod lsm_tree;
mod memtable;
mod service;

use crate::lsm_tree::compaction::CompactionCoordinator;
use crate::lsm_tree::compaction::CompactionEvents;
use crate::lsm_tree::disk::LsmTreeReader;
use crate::memtable::mem::{ConfigSource, Memtable};
use crate::service::init_logger;
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as TokioRwLock;

const USAGE: &str = "Usage: <./swift-merge> <path/to/config>";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logger()?;
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        log::error!("failed to pass in lsm-tree config file\n{}", USAGE);
        std::process::exit(1);
    }

    let table = match Memtable::new(ConfigSource::FileSource(&args[1])) {
        Ok(tb) => tb,
        Err(err) => {
            log::error!("failed to generate memtable {:?}", err);
            std::process::exit(1);
        }
    };

    log::info!("Config loaded successfully, starting application...");

    // Start gRPC Server
    let lsm_tree_reader_instance = Arc::new(TokioRwLock::new(LsmTreeReader::new().await?));
    let lsm_tree_reader_instance_clone = Arc::clone(&lsm_tree_reader_instance);

    let compaction = CompactionCoordinator::new(
        &table.config.as_ref().unwrap(),
        vec![(
            CompactionEvents::CompactionStarted,
            Box::new(move || {
                let lsm_tree = lsm_tree_reader_instance_clone.clone();
                tokio::spawn(async move {
                    let mut instance = lsm_tree.write().await;
                    let _ = instance.reload().await;
                });
            }),
        )],
    );

    let memtable_instance = Arc::new(RwLock::new(table));
    CompactionCoordinator::monitor(Arc::new(TokioRwLock::new(compaction)));

    let addr = "0.0.0.0:50051".parse()?;
    service::run_server(memtable_instance, lsm_tree_reader_instance, addr).await?;
    Ok(())
}
