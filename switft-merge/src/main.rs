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
    let mut example = std::collections::BTreeMap::new();
    example.insert(42, "zebra");
    example.insert(7, "apple");
    example.insert(99, "xylophone");
    example.insert(3, "banana");
    example.insert(150, "mango");
    example.insert(1, "cherry");
    example.insert(88, "kiwi");
    example.insert(25, "grape");
    example.insert(200, "orange");
    example.insert(15, "lemon");

    println!("Keys in insertion order: 42, 7, 99, 3, 150, 1, 88, 25, 200, 15");
    println!("BTreeMap iteration (should be sorted):");
    let sorted = example.iter().is_sorted();
    println!("is b-tree sorted {}", sorted);

    /*init_logger()?;
        let args: Vec<String> = env::args().collect();

        if args.len() < 2 {
            log::error!("failed to pass in lsm-tree config file\n {}", USAGE);
            std::process::exit(1);
        }

        let table = mem::Memtable::new(mem::ConfigSource::FileSource(&args[1])).unwrap();

        log::info!("Config loaded successfully, starting application...");

        // --- Start gRPC Server ---
        let db_instance = Arc::new(Mutex::new(table));

        let addr = "0.0.0.0:50051".parse()?;
        service::run_server(db_instance, addr).await?;
    */
    Ok(())
}
