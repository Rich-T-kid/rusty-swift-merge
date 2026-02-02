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
            expected.write_all(&1u32.to_le_bytes()).unwrap();
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
        use core::panic;
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
            let invalid_buffer = vec![16u8, 1u8, 2u8, 3u8];
            // buffer is short, expects value to be 16 bytes long but it only contains 4 bytes
            if let Ok(_) = TableEntry::deserialize(invalid_buffer) {
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

            if let Ok(tb) = TableEntry::deserialize(table_entry_serialization) {
                let meta_data = tb.meta_data.unwrap();
                let type_info = meta_data.get("host").unwrap();
                if type_info.true_type != TrueTypes::Unspecified {
                    panic!("invalid true type variant returned")
                }
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

            if let Ok(tb) = TableEntry::deserialize(table_entry_serialization) {
                let meta_data = tb.meta_data.unwrap();
                let type_info = meta_data.get("prod").unwrap();
                if type_info.true_type != TrueTypes::Unspecified {
                    panic!("invalid true type variant returned")
                }
            }
            Ok(())
        }
    }
    mod transitive_repr_deserialize {
        use std::io::Write;

        use crate::memtable::mem::{
            Memtable, MemtableError, TOMB_STONE_BYTE_REPRESENTATION, TableEntry, TransitiveRepr,
            WalEntry,
        };

        #[test]
        fn deserialize_round_trip_tombstone() {
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"test_key";
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key, WalEntry::Tombstone())
                .unwrap();

            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();

            let result = mg.get(key).unwrap();
            assert_eq!(*result, None); // Tombstone
        }

        #[test]
        fn deserialize_round_trip_value() {
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"my_key";
            let table_entry = TableEntry::new("my_value".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key, WalEntry::Value(&table_entry))
                .unwrap();
            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();

            let result = mg.get(key).unwrap();
            assert_eq!(*result, Some(table_entry));
        }

        #[test]
        fn deserialize_round_trip_value_with_metadata() {
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

            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key, WalEntry::Value(&table_entry))
                .unwrap();

            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();
            let result = mg.get(key).unwrap();
            assert_eq!(*result, Some(table_entry));
        }

        #[test]
        fn deserialize_round_trip_large_value() {
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"large_data_key";
            let large_value = vec![42u8; 1024]; // 1KB of data
            let table_entry = TableEntry::new(large_value, None);

            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key, WalEntry::Value(&table_entry))
                .unwrap();

            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();
            println!("{:?}", mg);
            let result = mg.get(key).unwrap();
            assert_eq!(*result, Some(table_entry));
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

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
        }

        #[test]
        fn test_key_size_larger_than_buffer() {
            // Buffer claims key is 100 bytes but buffer is too small
            let mut buffer: Vec<u8> = Vec::new();
            buffer.write_all(&100u32.to_le_bytes()).unwrap(); // key length = 100
            buffer.write_all(b"tiny").unwrap(); // only 4 bytes 

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
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

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
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

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
        }

        #[test]
        fn test_empty_buffer() {
            let buffer: Vec<u8> = Vec::new();

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_ok());
        }

        #[test]
        fn test_buffer_with_only_key_length() {
            // Buffer only contains key length prefix, nothing else
            let mut buffer: Vec<u8> = Vec::new();
            buffer.write_all(&5u32.to_le_bytes()).unwrap();

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
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

            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
        }

        #[test]
        fn test_zero_value_length() {
            // Edge case: zero-length value
            let mut buffer: Vec<u8> = Vec::new();
            let key = b"key";
            buffer.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
            buffer.write_all(key).unwrap();
            buffer.write_all(&0u32.to_le_bytes()).unwrap(); // value length = 0
            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
        }
    }
    mod multi_key_test {
        use std::io::Write;

        use crate::memtable::mem::{
            Memtable, MemtableError, TableEntry, TransitiveRepr, TrueTypes, TypeInfoMetadata,
            WalEntry,
        };
        use std::collections::BTreeMap;

        #[test]
        fn test_rebuild_with_three_simple_keys() {
            let mut buffer: Vec<u8> = Vec::new();

            // First key-value pair
            let key1 = b"user:1001";
            let table_entry1 = TableEntry::new("Alice".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key1, WalEntry::Value(&table_entry1))
                .unwrap();

            // Second key-value pair
            let key2 = b"user:1002";
            let table_entry2 = TableEntry::new("Bob".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key2, WalEntry::Value(&table_entry2))
                .unwrap();

            // Third key-value pair
            let key3 = b"user:1003";
            let table_entry3 = TableEntry::new("Charlie".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key3, WalEntry::Value(&table_entry3))
                .unwrap();
            // Rebuild memtable
            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();

            // Verify all three keys
            let result1 = mg.get(key1).unwrap();
            assert_eq!(*result1, Some(table_entry1));

            let result2 = mg.get(key2).unwrap();
            assert_eq!(*result2, Some(table_entry2));

            let result3 = mg.get(key3).unwrap();
            assert_eq!(*result3, Some(table_entry3));
        }

        #[test]
        fn test_rebuild_with_four_keys_with_metadata() {
            let mut buffer: Vec<u8> = Vec::new();

            // First key-value pair with metadata
            let key1 = b"metric:cpu";
            let mut meta1 = BTreeMap::new();
            meta1.insert(
                String::from("host"),
                TypeInfoMetadata::new("server-1".as_bytes().to_vec(), TrueTypes::String),
            );
            meta1.insert(
                String::from("port"),
                TypeInfoMetadata::new(8080i32.to_le_bytes().to_vec(), TrueTypes::Int32),
            );
            let table_entry1 = TableEntry::new("85.5%".as_bytes().to_vec(), Some(meta1));
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key1, WalEntry::Value(&table_entry1))
                .unwrap();

            // Second key-value pair with metadata
            let key2 = b"metric:memory";
            let mut meta2 = BTreeMap::new();
            meta2.insert(
                String::from("host"),
                TypeInfoMetadata::new("server-2".as_bytes().to_vec(), TrueTypes::String),
            );
            meta2.insert(
                String::from("region"),
                TypeInfoMetadata::new("us-west-2".as_bytes().to_vec(), TrueTypes::String),
            );
            let table_entry2 = TableEntry::new("4096MB".as_bytes().to_vec(), Some(meta2));
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key2, WalEntry::Value(&table_entry2))
                .unwrap();

            // Third key-value pair with metadata
            let key3 = b"metric:disk";
            let mut meta3 = BTreeMap::new();
            meta3.insert(
                String::from("host"),
                TypeInfoMetadata::new("server-3".as_bytes().to_vec(), TrueTypes::String),
            );
            meta3.insert(
                String::from("used"),
                TypeInfoMetadata::new(512000i64.to_le_bytes().to_vec(), TrueTypes::Int64),
            );
            let table_entry3 = TableEntry::new("2TB".as_bytes().to_vec(), Some(meta3));
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key3, WalEntry::Value(&table_entry3))
                .unwrap();

            // Fourth key-value pair with metadata
            let key4 = b"metric:network";
            let mut meta4 = BTreeMap::new();
            meta4.insert(
                String::from("host"),
                TypeInfoMetadata::new("server-4".as_bytes().to_vec(), TrueTypes::String),
            );
            meta4.insert(
                String::from("active"),
                TypeInfoMetadata::new(1u8.to_ne_bytes().to_vec(), TrueTypes::Bool),
            );
            let table_entry4 = TableEntry::new("100Mbps".as_bytes().to_vec(), Some(meta4));
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key4, WalEntry::Value(&table_entry4))
                .unwrap();

            // Rebuild memtable
            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();

            // Verify all four keys
            let result1 = mg.get(key1).unwrap();
            assert_eq!(*result1, Some(table_entry1));

            let result2 = mg.get(key2).unwrap();
            assert_eq!(*result2, Some(table_entry2));

            let result3 = mg.get(key3).unwrap();
            assert_eq!(*result3, Some(table_entry3));

            let result4 = mg.get(key4).unwrap();
            assert_eq!(*result4, Some(table_entry4));
        }

        #[test]
        fn test_rebuild_with_mixed_values_and_tombstones() {
            let mut buffer: Vec<u8> = Vec::new();

            // First key with value
            let key1 = b"session:abc123";
            let table_entry1 = TableEntry::new("active".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key1, WalEntry::Value(&table_entry1))
                .unwrap();

            // Second key with tombstone
            let key2 = b"session:def456";
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key2, WalEntry::Tombstone())
                .unwrap();

            // Third key with value
            let key3 = b"session:ghi789";
            let table_entry3 = TableEntry::new("pending".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key3, WalEntry::Value(&table_entry3))
                .unwrap();

            // Rebuild memtable
            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();

            // Verify all three keys
            let result1 = mg.get(key1).unwrap();
            assert_eq!(*result1, Some(table_entry1));

            let result2 = mg.get(key2).unwrap();
            assert_eq!(*result2, None); // Tombstone

            let result3 = mg.get(key3).unwrap();
            assert_eq!(*result3, Some(table_entry3));
        }

        #[test]
        fn test_rebuild_with_malformed_third_key() {
            let mut buffer: Vec<u8> = Vec::new();

            // First key - properly formed
            let key1 = b"config:timeout";
            let table_entry1 = TableEntry::new("30s".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key1, WalEntry::Value(&table_entry1))
                .unwrap();

            // Second key - properly formed
            let key2 = b"config:retries";
            let table_entry2 = TableEntry::new("3".as_bytes().to_vec(), None);
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key2, WalEntry::Value(&table_entry2))
                .unwrap();

            // Third key - malformed: corrupt value length
            // Write key properly
            let key3 = b"config:buffer";
            buffer
                .write_all(&(key3.len() as u32).to_le_bytes())
                .unwrap();
            buffer.write_all(key3).unwrap();

            // Malformed: claim value is 100 bytes but only provide 10
            buffer.write_all(&100u32.to_le_bytes()).unwrap(); // claims 100 bytes
            buffer.write_all(b"shortvalue").unwrap(); // only 10 bytes

            // Attempt to rebuild memtable - should fail
            let mut mg = Memtable::new().unwrap();
            let result = mg.rebuild_memtable(buffer);

            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                MemtableError::WriteAheadLog(_)
            ));
        }

        #[test]
        fn test_rebuild_with_four_tombstones() {
            let mut buffer: Vec<u8> = Vec::new();

            // First key - tombstone
            let key1 = b"deleted:user:5001";
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key1, WalEntry::Tombstone())
                .unwrap();

            // Second key - tombstone
            let key2 = b"deleted:user:5002";
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key2, WalEntry::Tombstone())
                .unwrap();

            // Third key - tombstone
            let key3 = b"deleted:user:5003";
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key3, WalEntry::Tombstone())
                .unwrap();

            // Fourth key - tombstone
            let key4 = b"deleted:user:5004";
            TransitiveRepr::new()
                .to_wal_entry(&mut buffer, key4, WalEntry::Tombstone())
                .unwrap();

            // Rebuild memtable
            let mut mg = Memtable::new().unwrap();
            mg.rebuild_memtable(buffer).unwrap();

            // Verify all four keys are tombstones
            let result1 = mg.get(key1).unwrap();
            assert_eq!(*result1, None);

            let result2 = mg.get(key2).unwrap();
            assert_eq!(*result2, None);

            let result3 = mg.get(key3).unwrap();
            assert_eq!(*result3, None);

            let result4 = mg.get(key4).unwrap();
            assert_eq!(*result4, None);
        }
    }
    mod memtable_recovery_test {
        use std::collections::BTreeMap;

        use crate::memtable::mem::{Memtable, TableEntry, TrueTypes, TypeInfoMetadata};

        #[test]
        fn test_memtable_persists_after_drop() {
            // Create first memtable and insert values
            let mut mg = Memtable::new().unwrap();

            mg.put(
                b"user:1001",
                TableEntry::new("Alice".as_bytes().to_vec(), None),
            )
            .unwrap();
            mg.put(
                b"user:1002",
                TableEntry::new("Bob".as_bytes().to_vec(), None),
            )
            .unwrap();
            mg.put(
                b"user:1003",
                TableEntry::new("Charlie".as_bytes().to_vec(), None),
            )
            .unwrap();
            mg.put(
                b"session:abc",
                TableEntry::new("active".as_bytes().to_vec(), None),
            )
            .unwrap();
            mg.put(
                b"session:def",
                TableEntry::new("inactive".as_bytes().to_vec(), None),
            )
            .unwrap();
            mg.put(
                b"config:timeout",
                TableEntry::new("30s".as_bytes().to_vec(), None),
            )
            .unwrap();

            let mut cpu_metadata = BTreeMap::new();
            cpu_metadata.insert(
                String::from("host"),
                TypeInfoMetadata::new("server-1".as_bytes().to_vec(), TrueTypes::String),
            );
            cpu_metadata.insert(
                String::from("port"),
                TypeInfoMetadata::new(8080i32.to_le_bytes().to_vec(), TrueTypes::Int32),
            );
            mg.put(
                b"metric:cpu",
                TableEntry::new("85.5%".as_bytes().to_vec(), Some(cpu_metadata)),
            )
            .unwrap();

            let mut memory_metadata = BTreeMap::new();
            memory_metadata.insert(
                String::from("host"),
                TypeInfoMetadata::new("server-2".as_bytes().to_vec(), TrueTypes::String),
            );
            memory_metadata.insert(
                String::from("region"),
                TypeInfoMetadata::new("us-west-2".as_bytes().to_vec(), TrueTypes::String),
            );
            mg.put(
                b"metric:memory",
                TableEntry::new("4096MB".as_bytes().to_vec(), Some(memory_metadata)),
            )
            .unwrap();

            // Drop the memtable
            drop(mg);

            // Create a new memtable and verify all keys still exist
            let mg2 = Memtable::new().unwrap();

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

            for key in keys {
                let result = mg2.get(key.as_bytes()).unwrap();
                assert!(result.is_some(), "Key {} should exist after recovery", key);
            }
        }
    }
}
