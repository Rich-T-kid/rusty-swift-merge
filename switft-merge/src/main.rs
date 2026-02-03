mod memtable;
mod service;

use crate::memtable::mem;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --- Partner's Recovery Tests ---
    println!("--- Running Recovery Tests ---");
    let keys = vec![
        "user:1001",
        "user:1002",
        "user:1003",
        "session:abc",
        "session:def",
        "config:timeout",
        "metric:cpu",
        "metric:memory",
    ];

    // we create a shared instance for both recovery tests and the server
    let db_instance = Arc::new(Mutex::new(mem::Memtable::new()?));

    {
        let table = db_instance.lock().unwrap();
        for key in &keys {
            match table.get(key.as_bytes()) {
                Ok(value) => {
                    println!("key:{key}\tvalue:{value:?}")
                }
                Err(_) => {
                    println!("failed to recover {key} from disk")
                }
            }
        }
    }
    println!("--- Recovery Tests Complete ---\n");

    // --- Playground / Mock Tests ---
    println!("--- Running Playground Tests ---");
    
    let mut md = BTreeMap::new();
    let meta_entry = mem::TypeInfoMetadata {
        raw: 21i32.to_le_bytes().to_vec(),
        true_type: mem::TrueTypes::Int32,
    };
    md.insert("region".to_string(), meta_entry);

    let example_entry = mem::TableEntry {
        value: "first mock test".as_bytes().to_vec(),
        meta_data: Some(md),
    };
    
    {
        let mut table = db_instance.lock().unwrap();
        let key = "richards_key".as_bytes();
        table.put(key, example_entry).unwrap();
        let output = table.get(key).unwrap();
        println!("Mock Get Result: {:?}", output);
    }

    println!("--- Playground Tests Complete ---\n");

    // --- Start gRPC Server ---
    // Uncomment the lines below to start the server instead of just running the test
    /*
    let addr = "[::1]:50051".parse()?;
    service::run_server(db_instance, addr).await?;
    */

    Ok(())
}
