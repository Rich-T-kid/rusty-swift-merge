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
}
mod deserialize_test {
    mod table_entry_deserialize {
        use crate::memtable::mem::{TrueTypes, TypeInfoMetadata};
        use std::collections::BTreeMap;

        use crate::memtable::mem::TableEntry;
        #[test]
        fn deserialize_round_trip_empty_md() -> Result<(), Box<dyn std::error::Error>> {
            let table_entry = TableEntry::new("one piece this week".as_bytes().to_vec(), None);
            let table_entry_serialization = table_entry.serialize()?;
            let resulting_deserilization = TableEntry::deserialize(table_entry_serialization)?;
            assert_eq!(table_entry, resulting_deserilization);
            Ok(())
        }
        #[test]
        fn deserialize_round_trip_md() -> Result<(), Box<dyn std::error::Error>> {
            let mut meta_data_table = BTreeMap::new();
            let k1 = "host";
            let k2 = "prod";
            let k3 = "request_rate/second";
            let raw_1 = "us-east-1".as_bytes().to_vec();
            let raw_2 = 1u8.to_ne_bytes().as_slice().to_vec();
            let raw_3 = 3042i32.to_le_bytes().as_slice().to_vec();
            meta_data_table.insert(
                String::from(k1),
                TypeInfoMetadata::new(raw_1, TrueTypes::String),
            );
            meta_data_table.insert(
                String::from(k2),
                TypeInfoMetadata::new(raw_2, TrueTypes::Bool),
            );
            meta_data_table.insert(
                String::from(k3),
                TypeInfoMetadata::new(raw_3, TrueTypes::Int32),
            );
            let value_bytes = "value_1";
            let table_entry = TableEntry::new(
                value_bytes.as_bytes().to_vec(),
                Some(meta_data_table.clone()),
            );
            let table_entry_serialization = table_entry.serialize()?;
            let resulting_deserilization = TableEntry::deserialize(table_entry_serialization)?;
            assert_eq!(table_entry, resulting_deserilization);
            Ok(())
        }
        #[test]
        fn test_invalid_bytes_deserialization() {
            let invalidBuffer = vec![16u8, 1u8, 2u8, 3u8]; // buffer is short, expects value to be 16 bytes long but it only contains 4 bytes
            if let Ok(_) = TableEntry::deserialize(invalidBuffer) {
                panic!("deserialize should have returned an error")
            }
        }
        #[test]
        fn test_invalid_enum_variant_deserialization() -> Result<(), Box<dyn std::error::Error>> {
            let mut meta_data_table = BTreeMap::new();
            let k1 = "host";
            let raw_1 = "us-east-1".as_bytes().to_vec();
            meta_data_table.insert(
                String::from(k1),
                TypeInfoMetadata::new(raw_1, TrueTypes::String),
            );
            let value_bytes = "value_1";
            let table_entry = TableEntry::new(
                value_bytes.as_bytes().to_vec(),
                Some(meta_data_table.clone()),
            );
            let mut table_entry_serialization = table_entry.serialize()?;
            // Corrupt the last byte (enum variant) to an invalid value
            let last_idx = table_entry_serialization.len() - 1;
            table_entry_serialization[last_idx] = 255u8; // Invalid enum variant

            if let Ok(_) = TableEntry::deserialize(table_entry_serialization) {
                panic!("deserialize should have returned an error for invalid enum variant")
            }
            Ok(())
        }
        #[test]
        fn test_invalid_enum_variant_multiple_keys() -> Result<(), Box<dyn std::error::Error>> {
            let mut meta_data_table = BTreeMap::new();
            let k1 = "host";
            let k2 = "prod";
            let raw_1 = "us-east-1".as_bytes().to_vec();
            let raw_2 = 1u8.to_ne_bytes().as_slice().to_vec();
            meta_data_table.insert(
                String::from(k1),
                TypeInfoMetadata::new(raw_1, TrueTypes::String),
            );
            meta_data_table.insert(
                String::from(k2),
                TypeInfoMetadata::new(raw_2, TrueTypes::Bool),
            );
            let value_bytes = "test_value";
            let table_entry = TableEntry::new(
                value_bytes.as_bytes().to_vec(),
                Some(meta_data_table.clone()),
            );
            let mut table_entry_serialization = table_entry.serialize()?;
            // Corrupt the last byte (enum variant of last metadata entry) to an invalid value
            let last_idx = table_entry_serialization.len() - 1;
            table_entry_serialization[last_idx] = 99u8; // Invalid enum variant

            if let Ok(_) = TableEntry::deserialize(table_entry_serialization) {
                panic!("deserialize should have returned an error for invalid enum variant")
            }
            Ok(())
        }
    }
    mod transitive_repr_deserialize {
        use std::io::Write;

        use crate::memtable::mem::{
            TOMB_STONE_BYTE_REPRESENTATION, TableEntry, TransitiveRepr, WalEntry,
        };

        #[test]
        fn deserialize_round_trip_tombstone() -> Result<(), Box<dyn std::error::Error>> {
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"test_key";
            TransitiveRepr::new().to_wal_entry(&mut buffer, key, WalEntry::Tombstone())?;

            let (deserialized_key, deserialized_value) = TransitiveRepr::from_wal_entry(buffer)?;

            assert_eq!(deserialized_key, key.to_vec());
            assert_eq!(deserialized_value, None); // Tombstone
            Ok(())
        }

        #[test]
        fn deserialize_round_trip_value() -> Result<(), Box<dyn std::error::Error>> {
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"my_key";
            let table_entry = TableEntry::new("my_value".as_bytes().to_vec(), None);
            TransitiveRepr::new().to_wal_entry(&mut buffer, key, WalEntry::Value(&table_entry))?;

            let (deserialized_key, deserialized_value) = TransitiveRepr::from_wal_entry(buffer)?;

            assert_eq!(deserialized_key, key.to_vec());
            assert_eq!(deserialized_value, Some(table_entry));
            Ok(())
        }

        #[test]
        fn deserialize_round_trip_value_with_metadata() -> Result<(), Box<dyn std::error::Error>> {
            use crate::memtable::mem::{TrueTypes, TypeInfoMetadata};
            use std::collections::BTreeMap;

            let mut buffer: Vec<u8> = Vec::new();
            let key = b"server_metric";

            let mut meta_data_table = BTreeMap::new();
            meta_data_table.insert(
                String::from("host"),
                TypeInfoMetadata::new("us-east-1".as_bytes().to_vec(), TrueTypes::String),
            );
            meta_data_table.insert(
                String::from("port"),
                TypeInfoMetadata::new(8080i32.to_le_bytes().to_vec(), TrueTypes::Int32),
            );

            let table_entry = TableEntry::new(
                "active_connections: 42".as_bytes().to_vec(),
                Some(meta_data_table),
            );

            TransitiveRepr::new().to_wal_entry(&mut buffer, key, WalEntry::Value(&table_entry))?;

            let (deserialized_key, deserialized_value) = TransitiveRepr::from_wal_entry(buffer)?;

            assert_eq!(deserialized_key, key.to_vec());
            assert_eq!(deserialized_value, Some(table_entry));
            Ok(())
        }

        #[test]
        fn deserialize_round_trip_large_value() -> Result<(), Box<dyn std::error::Error>> {
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"large_data_key";
            let large_value = vec![42u8; 1024]; // 1KB of data
            let table_entry = TableEntry::new(large_value, None);

            TransitiveRepr::new().to_wal_entry(&mut buffer, key, WalEntry::Value(&table_entry))?;

            let (deserialized_key, deserialized_value) = TransitiveRepr::from_wal_entry(buffer)?;

            assert_eq!(deserialized_key, key.to_vec());
            assert_eq!(deserialized_value, Some(table_entry));
            Ok(())
        }

        #[test]
        fn test_key_size_smaller_than_buffer() {
            // Buffer claims key is 10 bytes but only provides 5 bytes
            let mut buffer: Vec<u8> = Vec::new();
            buffer.write_all(&10u32.to_le_bytes()).unwrap(); // key length = 10
            buffer.write_all(b"short").unwrap(); // only 5 bytes
            buffer.write_all(&1u32.to_le_bytes()).unwrap(); // value length
            buffer
                .write_all(&TOMB_STONE_BYTE_REPRESENTATION.to_le_bytes())
                .unwrap();

            // TODO: When from_wal_entry is implemented, this should fail
            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!("deserialize should have returned an error for undersized key buffer")
            }
        }

        #[test]
        fn test_key_size_larger_than_buffer() {
            // Buffer claims key is 100 bytes but buffer is too small
            let mut buffer: Vec<u8> = Vec::new();
            buffer.write_all(&100u32.to_le_bytes()).unwrap(); // key length = 100
            buffer.write_all(b"tiny").unwrap(); // only 4 bytes 

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!("deserialize should have returned an error for oversized key claim")
            }
        }

        #[test]
        fn test_value_size_smaller_than_buffer() {
            // Proper key but value claims to be larger than actual data
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"key";
            buffer.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
            buffer.write_all(key).unwrap();
            buffer.write_all(&50u32.to_le_bytes()).unwrap(); // claims 50 bytes
            buffer.write_all(b"small_value").unwrap(); // only 11 bytes

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!("deserialize should have returned an error for undersized value buffer")
            }
        }

        #[test]
        fn test_value_size_larger_than_buffer() {
            // Value length exceeds remaining buffer
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"key";
            buffer.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
            buffer.write_all(key).unwrap();
            buffer.write_all(&1000u32.to_le_bytes()).unwrap(); // claims 1000 bytes
            buffer.write_all(b"val").unwrap(); // only 3 bytes

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!("deserialize should have returned an error for oversized value claim")
            }
        }

        #[test]
        fn test_empty_buffer() {
            let buffer: Vec<u8> = Vec::new();

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!("deserialize should have returned an error for empty buffer")
            }
        }

        #[test]
        fn test_buffer_with_only_key_length() {
            // Buffer only contains key length prefix, nothing else
            let mut buffer: Vec<u8> = Vec::new();
            buffer.write_all(&5u32.to_le_bytes()).unwrap();

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!("deserialize should have returned an error for incomplete buffer")
            }
        }

        #[test]
        fn test_zero_key_length() {
            // Edge case: zero-length key
            let mut buffer: Vec<u8> = Vec::new();
            buffer.write_all(&0u32.to_le_bytes()).unwrap(); // key length = 0
            buffer.write_all(&1u32.to_le_bytes()).unwrap(); // value length = 1
            buffer
                .write_all(&TOMB_STONE_BYTE_REPRESENTATION.to_le_bytes())
                .unwrap();

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!(
                    "deserialize should have returned an error for zero length keys are not valid"
                )
            }
        }

        #[test]
        fn test_zero_value_length() {
            // Edge case: zero-length value
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"key";
            buffer.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
            buffer.write_all(key).unwrap();
            buffer.write_all(&0u32.to_le_bytes()).unwrap(); // value length = 0

            if let Ok(_) = TransitiveRepr::from_wal_entry(buffer) {
                panic!(
                    "deserialize should have returned an error for zero length values are not valid"
                )
            }
        }
    }
}
