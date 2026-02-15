use crate::memtable::mem::{Memtable, TableEntry, TrueTypes, TypeInfoMetadata};
use log::{info, warn};
use simplelog::*;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status, transport::Server};

// import the generated rust code from proto
pub mod swiftmerge {
    tonic::include_proto!("swiftmerge.v01");
}

// pull in the server trait and message types from the generated code
use swiftmerge::lsmdb_server::{Lsmdb, LsmdbServer};
pub use swiftmerge::{
    DeleteRequest, GenericResponse, GetRequest, GetResponse, PutRequest, ReadMetricsRequest,
    ReadMetricsResponse, SupportedMetadataTypes, TypeInfo, WriteMetricsRequest,
    WriteMetricsResponse,
};

// Helper function to validate put requests
fn validate_put_request(req: &PutRequest) -> Result<(), Status> {
    if req.key.is_empty() {
        warn!("received Put request with empty key");
        return Err(Status::invalid_argument("key argument cannot be empty"));
    }

    if req.value.is_empty() {
        warn!("received Put request with empty value");
        return Err(Status::invalid_argument("value argument cannot be empty"));
    }

    if req.key.len() > u32::MAX as usize {
        warn!(
            "received Put request with key that is too large: size:{}",
            req.key.len()
        );
        return Err(Status::invalid_argument(format!(
            "max put key size is {}, received key of size {}",
            u32::MAX,
            req.key.len(),
        )));
    }

    if req.value.len() > u32::MAX as usize {
        warn!(
            "received Put request with value that is too large: size:{}",
            req.value.len()
        );
        return Err(Status::invalid_argument(format!(
            "max put value size is {}, received value of size {}",
            u32::MAX,
            req.value.len(),
        )));
    }

    Ok(())
}

// this struct holds our database state
pub struct MyLsmDb {
    // we wrap memtable in mutex for thread-safe access from grpc threads
    db: Arc<Mutex<Memtable>>,
}

impl MyLsmDb {
    // create a new instance of our service with a shared memtable
    pub fn new(db: Arc<Mutex<Memtable>>) -> Self {
        MyLsmDb { db }
    }
}

// implement the grpc service trait
#[tonic::async_trait]
impl Lsmdb for MyLsmDb {
    // handle put requests to insert or update data
    async fn put(&self, request: Request<PutRequest>) -> Result<Response<GenericResponse>, Status> {
        info!("Put request: {:?}", request);
        let req = request.into_inner();
        info!(
            "Put request: key: {:?} ({})\tvalue:{:?} ({})\tmetadata:{:?}\n",
            req.key,
            req.key.len(),
            req.value,
            req.value.len(),
            req.metadata
        );

        // Validate request
        validate_put_request(&req)?;

        // map grpc metadata to internal typeinfo metadata
        let mut internal_metadata = BTreeMap::new();
        for (key, meta) in req.metadata {
            // convert the proto enum to our internal rust enum
            let true_type = match SupportedMetadataTypes::try_from(meta.true_type) {
                Ok(SupportedMetadataTypes::Bool) => TrueTypes::Bool,
                Ok(SupportedMetadataTypes::RawByte) => TrueTypes::RawBytes,
                Ok(SupportedMetadataTypes::String) => TrueTypes::String,
                Ok(SupportedMetadataTypes::Uint32) => TrueTypes::Uint32,
                Ok(SupportedMetadataTypes::Uint64) => TrueTypes::Uint64,
                Ok(SupportedMetadataTypes::Int32) => TrueTypes::Int32,
                Ok(SupportedMetadataTypes::Int64) => TrueTypes::Int64,
                Ok(SupportedMetadataTypes::Float32) => TrueTypes::Float32,
                Ok(SupportedMetadataTypes::Double) => TrueTypes::Double,
                _ => TrueTypes::Unspecified,
            };

            internal_metadata.insert(key, TypeInfoMetadata::new(meta.raw, true_type));
        }

        // package the value and metadata into a table entry
        let entry = TableEntry::new(req.value, Some(internal_metadata));

        // lock the database and perform the write operation
        let mut db = self
            .db
            .lock()
            .map_err(|_| Status::internal("lock poisoned"))?;
        db.put(&req.key, entry)
            .map_err(|e| Status::internal(format!("db error: {:?}", e)))?;

        Ok(Response::new(GenericResponse {}))
    }

    // handle delete requests to remove data using tombstones
    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<GenericResponse>, Status> {
        let req = request.into_inner();
        info!("Delete request: key{:?}\n", req.key);

        // lock the database and write a tombstone for the key
        let mut db = self
            .db
            .lock()
            .map_err(|_| Status::internal("lock poisoned"))?;
        db.delete(&req.key)
            .map_err(|e| Status::internal(format!("db error: {:?}", e)))?;

        Ok(Response::new(GenericResponse {}))
    }

    // handle get requests to retrieve data
    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let req = request.into_inner();
        info!("Get request: key: {:?}\tfilter:{:?}\n", req.key, req.filter);

        // lock the database for reading
        let mut db = self
            .db
            .lock()
            .map_err(|_| Status::internal("lock poisoned"))?;

        // look up the key in the memtable
        match db.get(&req.key) {
            Ok(Some(entry)) => {
                // map internal metadata back to grpc typeinfo for the client
                let mut grpc_metadata = BTreeMap::new();
                if let Some(md) = &entry.meta_data {
                    for (key, meta) in md {
                        // convert our internal rust enum back to the proto enum
                        let grpc_type = match meta.true_type {
                            TrueTypes::Bool => SupportedMetadataTypes::Bool,
                            TrueTypes::RawBytes => SupportedMetadataTypes::RawByte,
                            TrueTypes::String => SupportedMetadataTypes::String,
                            TrueTypes::Uint32 => SupportedMetadataTypes::Uint32,
                            TrueTypes::Uint64 => SupportedMetadataTypes::Uint64,
                            TrueTypes::Int32 => SupportedMetadataTypes::Int32,
                            TrueTypes::Int64 => SupportedMetadataTypes::Int64,
                            TrueTypes::Float32 => SupportedMetadataTypes::Float32,
                            TrueTypes::Double => SupportedMetadataTypes::Double,
                            _ => SupportedMetadataTypes::Unspecified,
                        };

                        grpc_metadata.insert(
                            key.clone(),
                            TypeInfo {
                                raw: meta.raw.clone(),
                                true_type: grpc_type as i32,
                            },
                        );
                    }
                }

                // apply metadata filtering if requested by the client
                if let Some(filter) = req.filter {
                    if filter.r#use {
                        // keep only the keys specified in the filter
                        grpc_metadata.retain(|k, _| filter.metadata_keys.contains(k));
                    }
                }

                // return the value and metadata
                Ok(Response::new(GetResponse {
                    value: Some(entry.value.clone()),
                    metadata: grpc_metadata.into_iter().collect(),
                }))
            }
            Ok(None) => {
                // return an empty response if the key was deleted (tombstone)
                Ok(Response::new(GetResponse {
                    value: None,
                    metadata: HashMap::new(),
                }))
            }
            Err(_) => {
                // return an empty response if the key was not found
                Ok(Response::new(GetResponse {
                    value: None,
                    metadata: HashMap::new(),
                }))
            }
        }
    }

    // metrics are not yet tracked by the backend storage
    async fn write_metrics(
        &self,
        _request: Request<WriteMetricsRequest>,
    ) -> Result<Response<WriteMetricsResponse>, Status> {
        Err(Status::unimplemented(
            "metrics not yet implemented in backend",
        ))
    }

    // metrics are not yet tracked by the backend storage
    async fn read_metrics(
        &self,
        _request: Request<ReadMetricsRequest>,
    ) -> Result<Response<ReadMetricsResponse>, Status> {
        Err(Status::unimplemented(
            "metrics not yet implemented in backend",
        ))
    }
}
pub fn init_logger() -> Result<(), Box<dyn std::error::Error>> {
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Debug,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Info,
            Config::default(),
            File::options().append(true).create(true).open("app.log")?,
        ),
    ])?;
    Ok(())
}
// helper function to start the grpc server
pub async fn run_server(
    db: Arc<Mutex<Memtable>>,
    addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let lsm_db = MyLsmDb::new(db);

    info!("lsm-db grpc server listening on {}", addr);

    // build and run the server with our service implementation
    Server::builder()
        .add_service(LsmdbServer::new(lsm_db))
        .serve(addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod integration_tests {
    use tonic::transport::Channel;

    // Import the generated client code
    pub mod swiftmerge {
        tonic::include_proto!("swiftmerge.v01");
    }

    use swiftmerge::lsmdb_client::LsmdbClient;
    use swiftmerge::{DeleteRequest, GetRequest, PutRequest};

    const SERVER_ADDRESS: &str = "http://104.236.210.9:50051";

    #[tokio::test]
    async fn test_grpc_connection() -> Result<(), Box<dyn std::error::Error>> {
        // Connect to the gRPC server
        let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;

        let _ = LsmdbClient::new(channel);

        println!("Successfully connected to gRPC server");

        Ok(())
    }
    mod service_crud_operations {
        mod put_operations {
            use super::super::swiftmerge::lsmdb_client::LsmdbClient;
            use super::super::swiftmerge::{GetRequest, PutRequest};
            use super::super::{Channel, SERVER_ADDRESS};
            //crate::service
            #[tokio::test]
            async fn test_basic_put() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;

                let mut client = LsmdbClient::new(channel);

                // Put a value
                let put_request = PutRequest {
                    key: b"test_key".to_vec(),
                    value: b"test_value".to_vec(),
                    metadata: Default::default(),
                };

                let response = client.put(put_request.clone()).await?;
                println!("Put response: {:?}", response);

                // Get the value back
                let get_request = GetRequest {
                    key: b"test_key".to_vec(),
                    filter: None,
                };

                let response = client.get(get_request).await?;
                println!("Get response: {:?}", response);
                let inner_content = response.into_inner();
                assert_eq!(inner_content.value.unwrap(), put_request.value);
                Ok(())
            }
            // 5 -6 put request
            #[tokio::test]
            async fn test_multiple_put() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Put 6 different key-value pairs
                for i in 0..6 {
                    let key = format!("multi_key_{}", i);
                    let value = format!("multi_value_{}", i);

                    let put_request = PutRequest {
                        key: key.as_bytes().to_vec(),
                        value: value.as_bytes().to_vec(),
                        metadata: Default::default(),
                    };

                    let response = client.put(put_request).await?;
                    println!("Put response for {}: {:?}", key, response);
                }

                // Verify all values were stored
                for i in 0..6 {
                    let key = format!("multi_key_{}", i);
                    let expected_value = format!("multi_value_{}", i);

                    let get_request = GetRequest {
                        key: key.as_bytes().to_vec(),
                        filter: None,
                    };

                    let response = client.get(get_request).await?;
                    let inner = response.into_inner();
                    assert_eq!(inner.value.unwrap(), expected_value.as_bytes().to_vec());
                }

                Ok(())
            }
            //put -> read -> put/overwrite -> read (should be the overwritten value)
            #[tokio::test]
            async fn test_put_overwrite() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"overwrite_key".to_vec();
                let original_value = b"original_value".to_vec();
                let new_value = b"new_overwritten_value".to_vec();

                // Initial put
                let put_request = PutRequest {
                    key: key.clone(),
                    value: original_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Read to verify original value
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), original_value);

                // Overwrite with new value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: new_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Read again to verify overwritten value
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), new_value);

                Ok(())
            }
            // should return an error, puts with 0 bytes should return an error
            #[tokio::test]
            async fn test_put_empty_value() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Attempt to put with empty value
                let put_request = PutRequest {
                    key: b"test_key".to_vec(),
                    value: vec![], // empty value
                    metadata: Default::default(),
                };

                let result = client.put(put_request).await;

                // Should return an error
                assert!(result.is_err());
                let err = result.unwrap_err();
                assert_eq!(err.code(), tonic::Code::InvalidArgument);
                assert!(err.message().contains("value argument cannot be empty"));

                Ok(())
            }
            // first put should be 16kb and second entry should be 1mb, both should work
            #[tokio::test]
            async fn test_put_large_value() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Test 16KB value
                let large_value_16kb = vec![b'A'; 16 * 1024]; // 16KB
                let put_request = PutRequest {
                    key: b"large_key_16kb".to_vec(),
                    value: large_value_16kb.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Verify 16KB value
                let get_request = GetRequest {
                    key: b"large_key_16kb".to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), large_value_16kb);

                // Test 1MB value
                let large_value_1mb = vec![b'B'; 1024 * 1024]; // 1MB
                let put_request = PutRequest {
                    key: b"large_key_1mb".to_vec(),
                    value: large_value_1mb.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Verify 1MB value
                let get_request = GetRequest {
                    key: b"large_key_1mb".to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), large_value_1mb);

                Ok(())
            }
            // iteratre from 2000-3000 and make put request pretty simple
            #[tokio::test]
            async fn test_put_1k_keys() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Put 1000 keys (from 2000 to 2999)
                for i in 2000..3000 {
                    let key = format!("key_{}", i);
                    let value = format!("value_{}", i);

                    let put_request = PutRequest {
                        key: key.as_bytes().to_vec(),
                        value: value.as_bytes().to_vec(),
                        metadata: Default::default(),
                    };

                    client.put(put_request).await?;
                }

                println!("Successfully put 1000 keys");

                // Verify a few random keys
                for i in [2000, 2500, 2999] {
                    let key = format!("key_{}", i);
                    let expected_value = format!("value_{}", i);

                    let get_request = GetRequest {
                        key: key.as_bytes().to_vec(),
                        filter: None,
                    };

                    let response = client.get(get_request).await?;
                    assert_eq!(
                        response.into_inner().value.unwrap(),
                        expected_value.as_bytes().to_vec()
                    );
                }

                Ok(())
            }
        }
        mod get_operations {
            use super::super::swiftmerge::lsmdb_client::LsmdbClient;
            use super::super::swiftmerge::{GetRequest, PutRequest};
            use super::super::{Channel, SERVER_ADDRESS};

            // Put a key, then Get it, verify correct value returned
            #[tokio::test]
            async fn test_get_existing_key() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"get_test_key".to_vec();
                let value = b"get_test_value".to_vec();

                // Put a value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Get the value back
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();

                assert_eq!(inner.value.unwrap(), value);

                Ok(())
            }

            // Get a key that was never Put, verify appropriate response (empty/error)
            #[tokio::test]
            async fn test_get_non_existent_key() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Get a key that doesn't exist
                let get_request = GetRequest {
                    key: b"non_existent_key_12345".to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();

                // Should return None for non-existent keys
                assert!(inner.value.is_none());

                Ok(())
            }

            // Put key "x" three times with different values, verify Get returns latest
            #[tokio::test]
            async fn test_get_after_multiple_overwrites() -> Result<(), Box<dyn std::error::Error>>
            {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"overwrite_test".to_vec();

                // First put
                let put_request = PutRequest {
                    key: key.clone(),
                    value: b"first_value".to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Second put (overwrite)
                let put_request = PutRequest {
                    key: key.clone(),
                    value: b"second_value".to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Third put (overwrite again)
                let put_request = PutRequest {
                    key: key.clone(),
                    value: b"third_value".to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Get should return the latest value
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();

                assert_eq!(inner.value.unwrap(), b"third_value".to_vec());

                Ok(())
            }

            #[tokio::test]
            async fn test_get_empty_value() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // This test expects put with empty value to fail based on service logic
                // So we verify the error is returned
                let put_request = PutRequest {
                    key: b"empty_value_key".to_vec(),
                    value: vec![],
                    metadata: Default::default(),
                };

                let result = client.put(put_request).await;

                // Should return an error
                assert!(result.is_err());
                assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);

                Ok(())
            }

            // Put and Get in rapid succession, verify consistency
            #[tokio::test]
            async fn test_get_immediately_after_put() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                for i in 0..10 {
                    let key = format!("rapid_key_{}", i);
                    let value = format!("rapid_value_{}", i);

                    // Put
                    let put_request = PutRequest {
                        key: key.as_bytes().to_vec(),
                        value: value.as_bytes().to_vec(),
                        metadata: Default::default(),
                    };
                    client.put(put_request).await?;

                    // Immediately Get
                    let get_request = GetRequest {
                        key: key.as_bytes().to_vec(),
                        filter: None,
                    };
                    let response = client.get(get_request).await?;
                    let inner = response.into_inner();

                    // Verify consistency
                    assert_eq!(inner.value.unwrap(), value.as_bytes().to_vec());
                }

                Ok(())
            }
        }
        mod delete_operations {
            use super::super::swiftmerge::lsmdb_client::LsmdbClient;
            use super::super::swiftmerge::{DeleteRequest, GetRequest, PutRequest};
            use super::super::{Channel, SERVER_ADDRESS};

            // Put a key, Delete it, verify deletion success
            #[tokio::test]
            async fn test_delete_existing_key() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"delete_test_key".to_vec();
                let value = b"delete_test_value".to_vec();

                // Put a value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Delete the key
                let delete_request = DeleteRequest { key: key.clone() };
                let response = client.delete(delete_request).await?;

                println!("Delete response: {:?}", response);
                let get_req = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_req).await?;
                println!("get request after delete: {:?}", response);

                Ok(())
            }

            // Put a key, Delete it, Get it, verify key not found
            #[tokio::test]
            async fn test_delete_then_get() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"delete_get_test".to_vec();

                // Put a value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: b"some_value".to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Delete the key
                let delete_request = DeleteRequest { key: key.clone() };
                client.delete(delete_request).await?;

                // Get should return None (tombstone)
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();

                assert!(inner.value.is_none());

                Ok(())
            }

            // Delete a key that doesn't exist, verify appropriate response
            #[tokio::test]
            async fn test_delete_non_existent_key() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Delete a key that doesn't exist
                let delete_request = DeleteRequest {
                    key: b"non_existent_delete_key".to_vec(),
                };
                let response = client.delete(delete_request).await?;

                // Should succeed (deleting non-existent key is not an error)
                println!("Delete non-existent key response: {:?}", response);

                Ok(())
            }

            // Put key, Delete it, Put same key with new value, Get it, verify new value
            #[tokio::test]
            async fn test_delete_then_reput() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"reput_key".to_vec();
                let original_value = b"original_value".to_vec();
                let new_value = b"new_value_after_delete".to_vec();

                // Put original value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: original_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Delete it
                let delete_request = DeleteRequest { key: key.clone() };
                client.delete(delete_request).await?;

                // Put new value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: new_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Get should return new value
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();

                assert_eq!(inner.value.unwrap(), new_value);

                Ok(())
            }

            // Delete same key twice, verify both operations handle gracefully
            #[tokio::test]
            async fn test_delete_multiple_times() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"multi_delete_key".to_vec();

                // Put a value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: b"some_value".to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Delete first time
                let delete_request = DeleteRequest { key: key.clone() };
                let response1 = client.delete(delete_request).await?;
                println!("First delete response: {:?}", response1);

                // Delete second time (should succeed gracefully)
                let delete_request = DeleteRequest { key: key.clone() };
                let response2 = client.delete(delete_request).await?;
                println!("Second delete response: {:?}", response2);

                Ok(())
            }

            // Put key "a", Delete "a", Put "a" again, verify final value correct
            #[tokio::test]
            async fn test_put_delete_put_sequence() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                let key = b"a".to_vec();
                let first_value = b"first_value_for_a".to_vec();
                let final_value = b"final_value_for_a".to_vec();

                // Put first value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: first_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Delete it
                let delete_request = DeleteRequest { key: key.clone() };
                client.delete(delete_request).await?;

                // Put again with new value
                let put_request = PutRequest {
                    key: key.clone(),
                    value: final_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Verify final value is correct
                let get_request = GetRequest {
                    key: key.clone(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();

                assert_eq!(inner.value.unwrap(), final_value);

                Ok(())
            }
        }
    }
    mod edge_cases {
        use super::swiftmerge::lsmdb_client::LsmdbClient;
        use super::swiftmerge::{DeleteRequest, GetRequest, PutRequest};
        use super::{Channel, SERVER_ADDRESS};

        // Put keys 1-10, overwrite 1-5, Delete 6-8, verify final state
        #[tokio::test]
        async fn test_overwrite_pattern() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // Put keys 1-10
            for i in 1..=10 {
                let key = format!("key_{}", i);
                let value = format!("value_{}", i);

                let put_request = PutRequest {
                    key: key.as_bytes().to_vec(),
                    value: value.as_bytes().to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;
            }

            // Overwrite 1-5
            for i in 1..=5 {
                let key = format!("key_{}", i);
                let value = format!("overwritten_value_{}", i);

                let put_request = PutRequest {
                    key: key.as_bytes().to_vec(),
                    value: value.as_bytes().to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;
            }

            // Delete 6-8
            for i in 6..=8 {
                let key = format!("key_{}", i);
                let delete_request = DeleteRequest {
                    key: key.as_bytes().to_vec(),
                };
                client.delete(delete_request).await?;
            }

            // Verify final state
            // Keys 1-5 should have overwritten values
            for i in 1..=5 {
                let key = format!("key_{}", i);
                let expected_value = format!("overwritten_value_{}", i);

                let get_request = GetRequest {
                    key: key.as_bytes().to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(
                    response.into_inner().value.unwrap(),
                    expected_value.as_bytes().to_vec()
                );
            }

            // Keys 6-8 should be deleted (None)
            for i in 6..=8 {
                let key = format!("key_{}", i);
                let get_request = GetRequest {
                    key: key.as_bytes().to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert!(response.into_inner().value.is_none());
            }

            // Keys 9-10 should have original values
            for i in 9..=10 {
                let key = format!("key_{}", i);
                let expected_value = format!("value_{}", i);

                let get_request = GetRequest {
                    key: key.as_bytes().to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(
                    response.into_inner().value.unwrap(),
                    expected_value.as_bytes().to_vec()
                );
            }

            Ok(())
        }

        // Put/Delete same key 10 times alternating, verify final state
        #[tokio::test]
        async fn test_alternating_put_delete_same_key() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            let key = b"alternating_key".to_vec();

            // Alternate Put and Delete 10 times
            for i in 0..10 {
                // Put
                let put_request = PutRequest {
                    key: key.clone(),
                    value: format!("value_{}", i).as_bytes().to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Delete
                let delete_request = DeleteRequest { key: key.clone() };
                client.delete(delete_request).await?;
            }

            // Final state should be deleted (None)
            let get_request = GetRequest {
                key: key.clone(),
                filter: None,
            };
            let response = client.get(get_request).await?;
            assert!(response.into_inner().value.is_none());

            Ok(())
        }

        // Put 1000 keys, Delete all 1000, verify empty state
        #[tokio::test]
        async fn test_delete_all_after_bulk_insert() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // Put 1000 keys
            for i in 0..1000 {
                let key = format!("bulk_key_{}", i);
                let value = format!("bulk_value_{}", i);

                let put_request = PutRequest {
                    key: key.as_bytes().to_vec(),
                    value: value.as_bytes().to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;
            }

            println!("Put 1000 keys");

            // Delete all 1000 keys
            for i in 0..1000 {
                let key = format!("bulk_key_{}", i);
                let delete_request = DeleteRequest {
                    key: key.as_bytes().to_vec(),
                };
                client.delete(delete_request).await?;
            }

            println!("Deleted all 1000 keys");

            // Verify all are deleted
            for i in 0..1000 {
                let key = format!("bulk_key_{}", i);
                let get_request = GetRequest {
                    key: key.as_bytes().to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert!(response.into_inner().value.is_none());
            }

            println!("Verified all 1000 keys are deleted");

            Ok(())
        }

        // Put key with value "v1", overwrite with "v2", verify no trace of "v1"
        #[tokio::test]
        async fn test_overwrite_integrity() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            let key = b"integrity_key".to_vec();

            // Put with v1
            let put_request = PutRequest {
                key: key.clone(),
                value: b"v1".to_vec(),
                metadata: Default::default(),
            };
            client.put(put_request).await?;

            // Overwrite with v2
            let put_request = PutRequest {
                key: key.clone(),
                value: b"v2".to_vec(),
                metadata: Default::default(),
            };
            client.put(put_request).await?;

            // Get should return v2 only
            let get_request = GetRequest {
                key: key.clone(),
                filter: None,
            };
            let response = client.get(get_request).await?;
            let value = response.into_inner().value.unwrap();

            assert_eq!(value, b"v2".to_vec());
            assert_ne!(value, b"v1".to_vec());

            Ok(())
        }

        // Delete key "a", verify keys "b", "c" still present
        #[tokio::test]
        async fn test_delete_doesnt_affect_other_keys() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // Put keys a, b, c
            for key in ["a", "b", "c"] {
                let put_request = PutRequest {
                    key: key.as_bytes().to_vec(),
                    value: format!("value_{}", key).as_bytes().to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;
            }

            // Delete key "a"
            let delete_request = DeleteRequest { key: b"a".to_vec() };
            client.delete(delete_request).await?;

            // Verify "a" is deleted
            let get_request = GetRequest {
                key: b"a".to_vec(),
                filter: None,
            };
            let response = client.get(get_request).await?;
            assert!(response.into_inner().value.is_none());

            // Verify "b" and "c" still exist
            for key in ["b", "c"] {
                let get_request = GetRequest {
                    key: key.as_bytes().to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(
                    response.into_inner().value.unwrap(),
                    format!("value_{}", key).as_bytes().to_vec()
                );
            }

            Ok(())
        }

        mod size_limits {
            use super::super::swiftmerge::lsmdb_client::LsmdbClient;
            use super::super::swiftmerge::{GetRequest, PutRequest};
            use super::super::{Channel, SERVER_ADDRESS};

            // Test with largest acceptable value size
            #[tokio::test]
            async fn test_maximum_value_size() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Test with 10MB value (adjust based on your system limits)
                let large_value = vec![b'X'; 10 * 1024 * 1024]; // 10MB
                println!("size of value :{}", large_value.len());
                let put_request = PutRequest {
                    key: b"max_size_key".to_vec(),
                    value: large_value.clone(),
                    metadata: Default::default(),
                };
                let response = client.put(put_request).await;
                // Verify retrieval
                assert!(response.is_err());

                Ok(())
            }

            // Insert 10000 tiny key-value pairs (1 byte each)
            #[tokio::test]
            async fn test_many_small_keys() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Put 10000 tiny key-value pairs
                for i in 0..10000 {
                    let key = format!("tiny_{}", i);
                    let value = vec![b'a']; // 1 byte

                    let put_request = PutRequest {
                        key: key.as_bytes().to_vec(),
                        value: value.clone(),
                        metadata: Default::default(),
                    };
                    client.put(put_request).await?;
                }

                println!("Put 10000 tiny keys");

                // Verify a sample of keys
                for i in [0, 5000, 9999] {
                    let key = format!("tiny_{}", i);
                    let get_request = GetRequest {
                        key: key.as_bytes().to_vec(),
                        filter: None,
                    };
                    let response = client.get(get_request).await?;
                    assert_eq!(response.into_inner().value.unwrap(), vec![b'a']);
                }

                Ok(())
            }

            // Insert mix of tiny (1B), medium (1KB), large (100KB) values
            #[tokio::test]
            async fn test_mixed_size_values() -> Result<(), Box<dyn std::error::Error>> {
                let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
                let mut client = LsmdbClient::new(channel);

                // Put tiny value (1B)
                let tiny_value = vec![b'T'; 1];
                let put_request = PutRequest {
                    key: b"tiny".to_vec(),
                    value: tiny_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Put medium value (1KB)
                let medium_value = vec![b'M'; 1024];
                let put_request = PutRequest {
                    key: b"medium".to_vec(),
                    value: medium_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Put large value (100KB)
                let large_value = vec![b'L'; 100 * 1024];
                let put_request = PutRequest {
                    key: b"large".to_vec(),
                    value: large_value.clone(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;

                // Verify all values
                let get_request = GetRequest {
                    key: b"tiny".to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), tiny_value);

                let get_request = GetRequest {
                    key: b"medium".to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), medium_value);

                let get_request = GetRequest {
                    key: b"large".to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                assert_eq!(response.into_inner().value.unwrap(), large_value);

                Ok(())
            }
        }
    }
    mod in_memory_stress_test {
        // tbd later
    }
}
