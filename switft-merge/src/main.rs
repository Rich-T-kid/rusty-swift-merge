mod lsm_tree;
mod memtable;
mod service;
use crate::{memtable::mem, service::init_logger};
use std::{
    env,
    sync::{Arc, Mutex},
};
const USAGE: &str = "Usage: <./swift-merge> <path/to/config>";
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logger()?;
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        log::error!("failed to pass in lsm-tree config file\n {}", USAGE);
        std::process::exit(1);
    }

    let table = mem::Memtable::new()
        .and_then(|mut t| {
            t.update_config(mem::ConfigSource::FileSource(&args[1]))?;
            Ok(t)
        })
        .unwrap();

    log::info!("Config loaded successfully, starting application...");

    // --- Start gRPC Server ---
    let db_instance = Arc::new(Mutex::new(table));

    let addr = "0.0.0.0:50051".parse()?;
    service::run_server(db_instance, addr).await?;

    Ok(())
}
