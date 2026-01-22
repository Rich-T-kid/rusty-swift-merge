mod memtable;
use std::collections::BTreeMap;
fn main() {
    let wal = memtable::mem::WalManager::new("tmp.wal").unwrap();
    println!("wal:{:?}", wal);
    let mut sorted_map = BTreeMap::new();
    let tmp = vec![3u8, 9u8];
    sorted_map.insert("k2".as_bytes(), tmp);
    sorted_map.insert("k1".as_bytes(), 19u32.to_ne_bytes().to_vec());
    println!("{:?}", sorted_map)
}
