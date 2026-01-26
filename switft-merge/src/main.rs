mod memtable;
use crate::memtable::mem;
use std::collections::HashMap;
fn main() {
    let mut md = HashMap::new();
    let meta_entry = mem::TypeInfoMetadata {
        raw: 21u32.to_ne_bytes().to_vec(),
        true_type: mem::TrueTypes::Int32,
    };
    md.insert("region".to_string(), meta_entry);

    let exaple_entry = memtable::mem::TableEntry {
        value: "first mock test".as_bytes().to_vec(),
        meta_data: Some(md),
    };
    let mut table = memtable::mem::Memtable::new().unwrap();
    let key = "richards_key".as_bytes();
    table.put(key, exaple_entry).unwrap();
    let output = table.get(key).unwrap();
    println!("{output:?}");
}

/*
Put:key:"richard",value:"1",map:{"age":{bytes:21,type:int32},"favorite-food":{bytes:"sushi",type:string}}

get:key:"richard" -> value:"1" , map:{....} (all elements)

get:key:"richard", filter:{use:true,metadata_key:["favorite-food"]} -> value:"1",map:{"favorite-food":{bytes:"sushi",type:string}}

*/
