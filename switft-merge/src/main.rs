mod memtable;
mod service;

use crate::memtable::mem;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging

    log::info!("Starting application...");

    // --- Start gRPC Server ---
    // Uncomment the lines below to start the server instead of just running the test
    let db_instance = Arc::new(Mutex::new(mem::Memtable::new().unwrap()));

    let addr = "0.0.0.0:50051".parse()?;
    service::run_server(db_instance, addr).await?;

    Ok(())
}
