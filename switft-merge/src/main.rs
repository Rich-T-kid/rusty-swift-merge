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

/*
Put:key:"richard",value:"1",map:{"age":{bytes:21,type:int32},"favorite-food":{bytes:"sushi",type:string}}

get:key:"richard" -> value:"1" , map:{....} (all elements)

get:key:"richard", filter:{use:true,metadata_key:"favorite-food"} -> value:"1",map:{"favorite-food":{bytes:"sushi",type:string}}




*/
