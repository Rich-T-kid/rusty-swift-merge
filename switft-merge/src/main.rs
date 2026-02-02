mod memtable;
fn main() {
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

    let memtable = memtable::mem::Memtable::new().unwrap();
    for key in keys {
        match memtable.get(key.as_bytes()) {
            Ok(value) => {
                println!("key:{key}\tvalue:{value:?}")
            }
            Err(_) => {
                println!("failed to recover {key} from disk")
            }
        }
    }
}
