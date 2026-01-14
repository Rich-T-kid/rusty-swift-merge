use std::{any, collections::HashMap, time};

#[derive(Debug)]
struct Entry<'a, 'b, 'c> {
    key: &'a str,
    value: &'b [u8],
    meta_data: Option<HashMap<String, MetaDataValue<'c>>>,
    time_stamp: time::Instant,
}
struct EntryBuilder {}
impl EntryBuilder {
    fn new_entry_without_metadata<'a, 'b>(key: &'a str, value: &'b [u8]) -> Entry<'a, 'b, 'a> {
        Entry {
            key,
            value,
            meta_data: None,
            time_stamp: time::Instant::now(),
        }
    }
    fn new_entry_with_metadata<'a, 'b, 'c>(
        key: &'a str,
        value: &'b [u8],
        meta_data: HashMap<String, MetaDataValue<'c>>,
    ) -> Entry<'a, 'b, 'c> {
        Entry {
            key,
            value,
            meta_data: Some(meta_data),
            time_stamp: time::Instant::now(),
        }
    }
}

impl<'a, 'b, 'c> Entry<'a, 'b, 'c> {}
#[derive(Debug)]
enum MetaDataValue<'a> {
    StringVal(&'a str),
    IntVal(i32),
    FloatVal(f64),
    BoolVal(bool),
    RawBytes(&'a [u8]),
}
fn main() {
    let x = EntryBuilder::new_entry_without_metadata("key", "value".as_bytes());
    println!("{:?}", x)
}

/*

Need to set up HTTP Server (GRPC)
set up library code for http server to call
/ keep general - lsm tree should be the abstraction the callers user but the caller can choose
to use it for metrics, Entry can be a metric but allows caller to remain flexiable
struct Entry{
    key &str
    Value []Byte
    metadata Option<HashMap<&str,any>>
    timestamp Instant
}
    Write(m Metric) Success?

    / Query
    Read(name &str,count int) / filter by tags
    Read(metadataName &str,count int)  /
    Read(timespan Instant,count int)

*/
