mod memtable;
use crate::memtable::mem;
use std::collections::BTreeMap;
fn main() {
    let mut mg = memtable::mem::Memtable::new().unwrap();
    println!("(Pre memtable): {mg:?}");
    let content = mg.wal.drain().unwrap();
    mg.rebuild_memtable(content).unwrap();
    println!("(post memtable): {mg:?}");
}

/*
Put:key:"richard",value:"1",map:{"age":{bytes:21,type:int32},"favorite-food":{bytes:"sushi",type:string}}

get:key:"richard" -> value:"1" , map:{....} (all elements)

get:key:"richard", filter:{use:true,metadata_key:["favorite-food"]} -> value:"1",map:{"favorite-food":{bytes:"sushi",type:string}}

*/

/*
TODO: work on the write path of memtable (wal -> memory)
      work on the read path of memtable (thin abstraction over hashmap)



*/
