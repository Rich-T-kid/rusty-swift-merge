mod memtable;
fn main() {
    let mut mg = memtable::mem::Memtable::new().unwrap();
    println!("(Pre memtable): {mg:?}");
    let content = mg.wal.drain().unwrap();
    mg.rebuild_memtable(content).unwrap();
    println!("(post memtable): {mg:?}");
}
