#[cfg(test)]
mod serialize_test {
    mod table_entry_serialize_test {
        use std::{collections::BTreeMap, io::Write};

        use crate::memtable::mem::{
            self, META_DATA_MAP_DOESNT_EXIST, META_DATA_MAP_EXIST, TableEntry, TypeInfoMetadata,
        };
        #[test]
        fn test_true_types_encoding() {
            let type_1 = mem::TrueTypes::Bool;
            let type_2 = mem::TrueTypes::Int64;
            let type_3 = mem::TrueTypes::String;
            let result_1 = type_1.enum_variant_value();
            let result_2 = type_2.enum_variant_value();
            let result_3 = type_3.enum_variant_value();
            assert_eq!((1u8, 7u8, 3u8), (result_1, result_2, result_3))
        }
        #[test]
        fn test_value_prefix_serialization() {
            let value_bytes = "value_1";
            let example_entry = TableEntry::new(value_bytes.as_bytes().to_vec(), None);
            let mut expect = Vec::new();
            expect
                .write_all((value_bytes.len() as u32).to_le_bytes().as_slice())
                .unwrap();
            expect.write_all(value_bytes.as_bytes()).unwrap();
            expect
                .write_all(META_DATA_MAP_DOESNT_EXIST.to_le_bytes().as_slice())
                .unwrap();
            let out = example_entry.serialize().unwrap();
            println!("expect:{:?} , got: {:?}", expect, out);
            assert_eq!(expect, out)
        }
        #[test]
        fn test_string_key_serialization() {
            let mut meta_data_table = BTreeMap::new();
            let k1 = "host";
            let k2 = "prod";
            let k3 = "request_rate/second";
            let raw_1 = "us-east-1".as_bytes().to_vec();
            let raw_2 = 1u8.to_ne_bytes().as_slice().to_vec();
            let raw_3 = 3042i32.to_le_bytes().as_slice().to_vec();
            meta_data_table.insert(
                String::from(k1),
                TypeInfoMetadata::new(raw_1, mem::TrueTypes::String),
            );
            meta_data_table.insert(
                String::from(k2),
                TypeInfoMetadata::new(raw_2, mem::TrueTypes::Bool),
            );
            meta_data_table.insert(
                String::from(k3),
                TypeInfoMetadata::new(raw_3, mem::TrueTypes::Int32),
            );
            let value_bytes = "value_1";
            let example_entry = TableEntry::new(
                value_bytes.as_bytes().to_vec(),
                Some(meta_data_table.clone()),
            );
            let mut expect = Vec::new();
            expect
                .write_all((value_bytes.len() as u32).to_le_bytes().as_slice())
                .unwrap();
            expect.write_all(value_bytes.as_bytes()).unwrap();
            expect
                .write_all(META_DATA_MAP_EXIST.to_le_bytes().as_slice())
                .unwrap();
            // key value pairs
            for (k, v) in meta_data_table {
                // key-len | key | raw-len | raw | enum_varient
                expect
                    .write_all((k.len() as u32).to_le_bytes().as_slice())
                    .unwrap();
                expect.write_all(k.as_bytes()).unwrap();
                expect
                    .write_all((v.raw.len() as u32).to_le_bytes().as_slice())
                    .unwrap();
                expect.write_all(&v.raw).unwrap();
                expect
                    .write_all(vec![v.true_type.enum_variant_value()].as_slice())
                    .unwrap();
            }
            let result = example_entry.serialize().unwrap();
            println!("expect:{:?}\nresult:{:?}", expect, result);
            assert_eq!(expect, result)
        }
        #[test]
        fn test_raw_bytes_serialization() {
            let raw_string = "us-east-1".as_bytes().to_vec();
            let raw_bool = 1u8.to_ne_bytes().as_slice().to_vec();
            let raw_int32 = 3042i32.to_le_bytes().as_slice().to_vec();
            let raw_int64 = 9999i64.to_le_bytes().as_slice().to_vec();

            let metadata_1 = TypeInfoMetadata::new(raw_string.clone(), mem::TrueTypes::String);
            let metadata_2 = TypeInfoMetadata::new(raw_bool.clone(), mem::TrueTypes::Bool);
            let metadata_3 = TypeInfoMetadata::new(raw_int32.clone(), mem::TrueTypes::Int32);
            let metadata_4 = TypeInfoMetadata::new(raw_int64.clone(), mem::TrueTypes::Int64);

            assert_eq!(metadata_1.raw, raw_string);
            assert_eq!(metadata_2.raw, raw_bool);
            assert_eq!(metadata_3.raw, raw_int32);
            assert_eq!(metadata_4.raw, raw_int64);
        }
        #[test]
        fn test_string_encoding_to_enum_encoding() {
            // Test ALL variants - this will break if any variant encoding changes
            assert_eq!(mem::TrueTypes::Unspecified.enum_variant_value(), 0u8);
            assert_eq!(mem::TrueTypes::Bool.enum_variant_value(), 1u8);
            assert_eq!(mem::TrueTypes::RawBytes.enum_variant_value(), 2u8);
            assert_eq!(mem::TrueTypes::String.enum_variant_value(), 3u8);
            assert_eq!(mem::TrueTypes::Uint32.enum_variant_value(), 4u8);
            assert_eq!(mem::TrueTypes::Uint64.enum_variant_value(), 5u8);
            assert_eq!(mem::TrueTypes::Int32.enum_variant_value(), 6u8);
            assert_eq!(mem::TrueTypes::Int64.enum_variant_value(), 7u8);
            assert_eq!(mem::TrueTypes::Float32.enum_variant_value(), 8u8);
            assert_eq!(mem::TrueTypes::Double.enum_variant_value(), 9u8);
        }
    }
    mod transitive_repr_serialize_test {
        use std::io::Write;

        use crate::memtable::mem::{
            TOMB_STONE_BYTE_REPRESENTATION, TableEntry, TransitiveRepr, WalEntry,
        };

        // check that key len prefix stuff works as it should
        // check for tombstone case
        #[test]
        fn test_key_prefix() {
            let mut buffer: Vec<u8> = Vec::new();
            let key_1 = &1u8.to_le_bytes();
            let value = WalEntry::Tombstone();
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key_1, value)
                .unwrap();
            let mut expected = Vec::new();
            expected
                .write_all((key_1.len() as u32).to_le_bytes().as_slice())
                .unwrap();
            expected.write_all(key_1).unwrap();
            expected.write_all(&1u8.to_le_bytes()).unwrap();
            expected
                .write_all(&TOMB_STONE_BYTE_REPRESENTATION.to_le_bytes())
                .unwrap();
            println!("expected:{:?}\nbuffer:{:?}", expected, buffer);
            assert_eq!(expected, buffer)
        }
        #[test]
        fn test_value_encoding() {
            let mut buffer: Vec<u8> = Vec::new();
            let key_1 = &1u8.to_le_bytes();
            let tb_entry = TableEntry::new("one piece is great".as_bytes().to_vec(), None);
            let serialized = tb_entry.serialize().unwrap();
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key_1, WalEntry::Value(&tb_entry))
                .unwrap();
            let mut expected = Vec::new();
            expected
                .write_all((key_1.len() as u32).to_le_bytes().as_slice())
                .unwrap();
            expected.write_all(key_1).unwrap();
            expected
                .write_all((serialized.len() as u32).to_le_bytes().as_slice())
                .unwrap();
            expected.write_all(&serialized).unwrap();
            println!("expected:{:?}\nbuffer:{:?}", expected, buffer);
            assert_eq!(expected, buffer)
        }

        // check the value len is correct
    }
    mod table_entry_deserialize {}
}
