use crate::memtable::mem::{Memtable, MemtableError, TableEntry, TrueTypes, TypeInfoMetadata};
use crate::service::swiftmerge::{ReadStatsResponse, WriteMetrics};
use log::{info, warn};
use simplelog::*;
use std::cmp::min;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::sync::{Arc, RwLock};
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock as TokioRwLock;
use tonic::{Request, Response, Status, transport::Server};

use crate::lsm_tree::disk::LsmTreeReader;
// import the generated rust code from proto
pub mod swiftmerge {
    tonic::include_proto!("swiftmerge.v01");
}

// pull in the server trait and message types from the generated code
use swiftmerge::lsmdb_server::{Lsmdb, LsmdbServer};
pub use swiftmerge::{
    DeleteRequest, GenericResponse, GetRequest, GetResponse, HealthCheckResponse, PutRequest,
    ReadMetricsRequest, ReadMetricsResponse, SupportedMetadataTypes, TypeInfo, WriteMetricsRequest,
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
    // we wrap memtable in read-write-lock for thread-safe access from grpc threads
    // lsm-tree is write heavy in nature so it make sense t
    memtable_mutex: Arc<RwLock<Memtable>>,
    lsm_tree: Arc<TokioRwLock<LsmTreeReader>>,
}

impl MyLsmDb {
    // create a new instance of our service with a shared memtable
    pub fn new(mem: Arc<RwLock<Memtable>>, lsm: Arc<TokioRwLock<LsmTreeReader>>) -> Self {
        MyLsmDb {
            memtable_mutex: mem,
            lsm_tree: lsm,
        }
    }
}

// implement the grpc service trait
#[tonic::async_trait]
impl Lsmdb for MyLsmDb {
    // handle put requests to insert or update data
    async fn put(&self, request: Request<PutRequest>) -> Result<Response<GenericResponse>, Status> {
        let req = request.into_inner();

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
            .memtable_mutex
            .write()
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
        let key_display = &req.key[..min(req.key.len(), 200)];
        info!("Delete request: key{:?}\n", key_display);

        // lock the database and write a tombstone for the key
        let mut db = self
            .memtable_mutex
            .write()
            .map_err(|_| Status::internal("lock poisoned"))?;
        db.delete(&req.key)
            .map_err(|e| Status::internal(format!("db error: {:?}", e)))?;

        Ok(Response::new(GenericResponse {}))
    }

    // handle get requests to retrieve data
    // ! slight API change, if the key doesnt exist in the memtable, check disk but this will be through a disk reader not the memtable directly, this is to deal with lock contention
    async fn get(&self, request: Request<GetRequest>) -> Result<Response<GetResponse>, Status> {
        let req = request.into_inner();
        let key_display = &req.key[..min(req.key.len(), 200)];
        info!(
            "Get request: key: {:?}\tfilter:{:?}\n",
            key_display, req.filter
        );

        // First, try to get from memtable in its own scope
        let memtable_result = {
            let db = self
                .memtable_mutex
                .read()
                .map_err(|_| Status::internal("lock poisoned"))?;

            db.get(&req.key).map(|opt| opt.as_ref().cloned())
        }; // Lock is dropped here

        // Now handle the result without holding the lock
        match memtable_result {
            Ok(Some(entry)) => {
                // map internal metadata back to grpc typeinfo for the client
                let mut grpc_metadata = BTreeMap::new();
                if let Some(md) = &entry.meta_data {
                    for (key, meta) in md {
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
                        grpc_metadata.retain(|k, _| filter.metadata_keys.contains(k));
                    }
                }

                Ok(Response::new(GetResponse {
                    value: Some(entry.value.clone()),
                    metadata: grpc_metadata.into_iter().collect(),
                }))
            }
            Ok(None) => {
                // Tombstone
                Ok(Response::new(GetResponse {
                    value: None,
                    metadata: HashMap::new(),
                }))
            }
            Err(variant) => {
                // Key not found in memtable, search on disk
                match variant {
                    MemtableError::MissingKey() => {
                        // Now safe to await since lock is already dropped
                        match self.lsm_tree.read().await.read(&req.key).await {
                            Ok(crate::lsm_tree::disk::SearchResult::Found(
                                value,
                                (_ss_tables, _levels),
                            )) => {
                                if value.len() == 1 {
                                    // tombstone
                                    return Ok(Response::new(GetResponse {
                                        value: None,
                                        metadata: HashMap::new(),
                                    }));
                                }
                                Ok(Response::new(GetResponse {
                                    value: Some(value),
                                    metadata: HashMap::new(),
                                }))
                            }
                            Ok(crate::lsm_tree::disk::SearchResult::Missing((
                                _ss_tables,
                                _levels,
                            ))) => Ok(Response::new(GetResponse {
                                value: None,
                                metadata: HashMap::new(),
                            })),
                            Err(e) => Err(Status::internal(format!("disk read error: {:?}", e))),
                        }
                    }
                    _ => Err(Status::internal(format!(
                        "error fetching key {:?}",
                        variant
                    ))),
                }
            }
        }
    }

    // metrics are not yet tracked by the backend storage
    async fn write_metrics(
        &self,
        _request: Request<WriteMetricsRequest>,
    ) -> Result<Response<WriteMetricsResponse>, Status> {
        let req = _request.into_inner();
        info!("write-metrics request recievied {:?}", req);
        let (mem_metrics, disk_metrics) = self
            .memtable_mutex
            .read()
            .map_err(|_| Status::internal("lock poisoned"))?
            .metrics();

        Ok(Response::new(WriteMetricsResponse {
            write_response: Some(WriteMetrics {
                total_writes: mem_metrics.memtable_writes,
                ss_table_count: disk_metrics.ss_table_count,
                ss_table_merged: disk_metrics.total_ss_tables_merged,
            }),
            avg_response: None,
        }))
    }

    // metrics are not yet tracked by the backend storage
    async fn read_metrics(
        &self,
        _request: Request<ReadMetricsRequest>,
    ) -> Result<Response<ReadMetricsResponse>, Status> {
        let req = _request.into_inner();
        info!("read-metrics request recievied {:?}", req);
        let (mem_metrics, _) = self
            .memtable_mutex
            .read()
            .map_err(|_| Status::internal("lock poisoned"))?
            .metrics();

        Ok(Response::new(ReadMetricsResponse {
            read_response: Some(ReadStatsResponse {
                memtable_reads: mem_metrics.memtable_reads,
                lsm_tree_reads: mem_metrics.lsm_reads,
                total_misses: mem_metrics.total_misses,
            }),
            avg_response: None,
        }))
    }

    async fn health_check(
        &self,
        _request: Request<()>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        use prost_types::Timestamp;
        use std::time::SystemTime;

        info!("Health check request received");

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| Status::internal(format!("time error: {}", e)))?;

        Ok(Response::new(HealthCheckResponse {
            time_stamp: Some(Timestamp {
                seconds: now.as_secs() as i64,
                nanos: now.subsec_nanos() as i32,
            }),
        }))
    }
    async fn batch_write(
        &self,
        _request: Request<swiftmerge::BatchWriteRequest>,
    ) -> Result<Response<GenericResponse>, Status> {
        let req = _request.into_inner();
        info!(
            "Batch write request: {} puts, {} deletes",
            req.puts.len(),
            req.deletes.len()
        );

        // Collect all delete keys into a mutable HashSet
        let mut delete_keys: HashSet<Vec<u8>> = req
            .deletes
            .iter()
            .map(|del_req| del_req.key.clone())
            .collect();

        let mut db = self.memtable_mutex.write().unwrap();

        // Process all puts
        for put_request in req.puts {
            // Convert metadata from proto TypeInfo to internal TypeInfoMetadata
            let mut internal_metadata = BTreeMap::new();
            for (key, meta) in put_request.metadata {
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

            let entry = TableEntry::new(put_request.value, Some(internal_metadata));
            db.put(&put_request.key, entry)
                .map_err(|e| Status::internal(format!("db error: {:?}", e)))?;

            // Remove from delete set if it was scheduled for deletion (put takes precedence)
            delete_keys.remove(&put_request.key);
        }

        // Process remaining deletes
        for delete_key in delete_keys {
            db.delete(&delete_key)
                .map_err(|e| Status::internal(format!("db error: {:?}", e)))?;
        }

        Ok(Response::new(GenericResponse {}))
    }

    // ! for now range searches are only for memtable, in the future for range reads on disk itd be similar to get where we get the results from the lsmTreeManager
    async fn range(
        &self,
        request: Request<swiftmerge::RangeRequest>,
    ) -> Result<Response<swiftmerge::RangeResponse>, Status> {
        let req = request.into_inner();
        let start_key_display = &req.start_key[..min(req.start_key.len(), 200)];
        let end_key_display = &req.end_key[..min(req.end_key.len(), 200)];
        info!(
            "Range request: start_key: {:?}, end_key: {:?}, filter: {:?}\n",
            start_key_display, end_key_display, req.filter
        );

        // Validate that start_key <= end_key
        if req.start_key > req.end_key {
            return Err(Status::invalid_argument(
                "start_key must be less than or equal to end_key",
            ));
        }

        // Lock the database for reading
        let db = self
            .memtable_mutex
            .read()
            .map_err(|_| Status::internal("lock poisoned"))?;

        // Perform range query
        let range_results = db
            .range(&req.start_key, &req.end_key)
            .map_err(|e| Status::internal(format!("range query error: {:?}", e)))?;

        // Convert results to RangeResponse
        let mut results = Vec::new();

        for (key, entry_opt) in range_results.iter() {
            // Skip tombstones (None entries)
            if entry_opt.is_none() {
                continue;
            }

            if let Some(entry) = entry_opt {
                // Map internal metadata to gRPC TypeInfo
                let mut grpc_metadata = BTreeMap::new();
                if let Some(md) = &entry.meta_data {
                    for (key, meta) in md {
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

                // Apply metadata filtering if requested
                // ! tbd

                results.push(swiftmerge::range_response::KeyValuePair {
                    key: (*key).clone(),
                    value: entry.value.clone(),
                    metadata: grpc_metadata.into_iter().collect(),
                });
            }
        }

        Ok(Response::new(swiftmerge::RangeResponse { results }))
    }

    async fn graceful_shutdown(
        &self,
        _request: Request<()>,
    ) -> Result<Response<GenericResponse>, Status> {
        info!("Graceful shutdown request received");
        match self
            .memtable_mutex
            .write()
            .map_err(|e| Status::internal(format!("failed to acquire lock on database: {:?}", e)))?
            .shutdown()
        {
            Ok(()) => {}
            Err(error) => return Err(Status::internal(format!("shutdown error: {:?}", error))),
        };

        let response = Ok(Response::new(GenericResponse {}));

        // Spawn a task to exit after giving time for the response to be sent
        tokio::spawn(async {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            std::process::exit(0);
        });

        response
    }
    async fn reload_config(
        &self,
        request: Request<swiftmerge::ReloadConfigRequest>,
    ) -> Result<Response<GenericResponse>, Status> {
        let req = request.into_inner();
        info!("Reload config request: {} bytes", req.json_config.len());

        let mut db = self.memtable_mutex.write().map_err(|e| {
            Status::internal(format!("failed to acquire lock on database: {:?}", e))
        })?;

        db.update_config(crate::memtable::mem::ConfigSource::RawBytes(
            req.json_config,
        ))
        .map_err(|e| Status::internal(format!("failed to update config: {:?}", e)))?;

        Ok(Response::new(GenericResponse {}))
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
    memtable: Arc<RwLock<Memtable>>,
    lsm_tree: Arc<TokioRwLock<LsmTreeReader>>,
    addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let lsm_db = MyLsmDb::new(memtable, lsm_tree);

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

    //const SERVER_ADDRESS: &str = "http://104.236.210.9:50051";
    const SERVER_ADDRESS: &str = "http://localhost:50051";

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

    mod additional_rpc_tests {
        use super::swiftmerge::lsmdb_client::LsmdbClient;
        use super::swiftmerge::{
            BatchWriteRequest, DeleteRequest, PutRequest, RangeRequest, ReloadConfigRequest,
        };
        use super::{Channel, SERVER_ADDRESS};

        // Test health_check RPC
        #[tokio::test]
        async fn test_health_check() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // Call health_check
            let response = client.health_check(()).await?;
            let health_response = response.into_inner();

            println!("Health check response: {:?}", health_response);

            // Verify that we got a timestamp
            assert!(health_response.time_stamp.is_some());
            let timestamp = health_response.time_stamp.unwrap();
            assert!(timestamp.seconds > 0);

            Ok(())
        }

        // Test reload_config RPC
        #[tokio::test]
        async fn test_reload_config() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // Create a valid JSON config
            let config_json = r#"{
                "wal_file_path": "./test_wal.log",
                "max_memtable_size_bytes": 1048576,
                "disk_tree_file_path": "./test_tree.db",
                "ramMaxSize": 20480,
                "ramMaxTime": 600,
                "targetChunks": 4,
                "compactionCheckIntervalSeconds": 3600,
                "walEnabled": true,
                "bloomFalsePositiveRate": 0.05,
                "maxCompactionThreads": 2,
                "localDisk": true
            }"#;

            let reload_request = ReloadConfigRequest {
                json_config: config_json.as_bytes().to_vec(),
            };

            // Call reload_config
            let response = client.reload_config(reload_request).await?;
            let reload_response = response.into_inner();

            println!("Reload config response: {:?}", reload_response);

            Ok(())
        }

        // Test batch_write RPC
        #[tokio::test]
        async fn test_batch_write() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // Prepare multiple put requests
            let mut puts = vec![];
            for i in 0..5 {
                puts.push(PutRequest {
                    key: format!("batch_key_{}", i).as_bytes().to_vec(),
                    value: format!("batch_value_{}", i).as_bytes().to_vec(),
                    metadata: Default::default(),
                });
            }

            // Prepare multiple delete requests
            let mut deletes = vec![];
            for i in 5..8 {
                deletes.push(DeleteRequest {
                    key: format!("batch_key_{}", i).as_bytes().to_vec(),
                });
            }

            // Create batch write request
            let batch_request = BatchWriteRequest {
                puts: puts.clone(),
                deletes: deletes.clone(),
            };

            // Call batch_write
            let response = client.batch_write(batch_request).await?;
            let batch_response = response.into_inner();

            println!("Batch write response: {:?}", batch_response);

            // Verify that the puts were successful
            for i in 0..5 {
                let get_request = super::swiftmerge::GetRequest {
                    key: format!("batch_key_{}", i).as_bytes().to_vec(),
                    filter: None,
                };
                let response = client.get(get_request).await?;
                let inner = response.into_inner();
                assert_eq!(
                    inner.value.unwrap(),
                    format!("batch_value_{}", i).as_bytes().to_vec()
                );
            }

            Ok(())
        }

        // Test batch_write with conflicting operations (put and delete same key)
        #[tokio::test]
        async fn test_batch_write_conflict_resolution() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            let conflicting_key = b"conflict_key".to_vec();

            // Create a batch write with both put and delete for the same key
            let batch_request = BatchWriteRequest {
                puts: vec![PutRequest {
                    key: conflicting_key.clone(),
                    value: b"conflict_value".to_vec(),
                    metadata: Default::default(),
                }],
                deletes: vec![DeleteRequest {
                    key: conflicting_key.clone(),
                }],
            };

            // Call batch_write (put should take precedence)
            let response = client.batch_write(batch_request).await?;
            println!("Batch write with conflict response: {:?}", response);

            // Verify that the key exists (put took precedence over delete)
            let get_request = super::swiftmerge::GetRequest {
                key: conflicting_key.clone(),
                filter: None,
            };
            let response = client.get(get_request).await?;
            let inner = response.into_inner();
            assert_eq!(inner.value.unwrap(), b"conflict_value".to_vec());

            Ok(())
        }

        // Test range RPC
        #[tokio::test]
        #[ignore] // Ignore this test for now as requested
        async fn test_range() -> Result<(), Box<dyn std::error::Error>> {
            let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
            let mut client = LsmdbClient::new(channel);

            // First, put some keys in a range
            for i in 100..110 {
                let put_request = PutRequest {
                    key: format!("range_key_{:03}", i).as_bytes().to_vec(),
                    value: format!("range_value_{}", i).as_bytes().to_vec(),
                    metadata: Default::default(),
                };
                client.put(put_request).await?;
            }

            // Create range request
            let range_request = RangeRequest {
                start_key: b"range_key_100".to_vec(),
                end_key: b"range_key_110".to_vec(),
                filter: None,
            };

            // Call range (should return unimplemented for now)
            let result = client.range(range_request).await;

            // For now, expect unimplemented error
            if let Err(status) = result {
                assert_eq!(status.code(), tonic::Code::Unimplemented);
                println!(
                    "Range RPC correctly returns unimplemented: {}",
                    status.message()
                );
            } else {
                // When implemented, verify the response
                let response = result.unwrap();
                let range_response = response.into_inner();
                println!("Range response: {:?}", range_response);
                assert_eq!(range_response.results.len(), 10);
            }

            Ok(())
        }
    }
}

#[cfg(test)]
mod generate_test_data {
    use std::time::Duration;
    use std::{clone, thread};

    use super::*;
    use crate::*;
    use tonic::Request;
    use tonic::transport::Channel;

    const SERVER_ADDRESS: &str = "http://104.236.210.9:50051";

    #[tokio::test]
    #[ignore]
    async fn generate_multiple_sstables() -> Result<(), Box<dyn std::error::Error>> {
        // Import swiftmerge client
        use super::swiftmerge::lsmdb_client::LsmdbClient;

        // Connect to gRPC server
        let channel = Channel::from_static(SERVER_ADDRESS).connect().await?;
        let mut client = LsmdbClient::new(channel);
        let count = 25;
        for i in 0..count {
            // Large list of keys to trigger multiple flushes
            let test_keys = vec![
                // Superheroes
                "spider_man",
                "iron_man",
                "captain_america",
                "thor",
                "hulk",
                "black_widow",
                "hawkeye",
                "doctor_strange",
                "black_panther",
                "ant_man",
                "wasp",
                "vision",
                "scarlet_witch",
                "winter_soldier",
                "falcon",
                "war_machine",
                "star_lord",
                "gamora",
                "drax",
                "rocket_raccoon",
                "groot",
                "mantis",
                "nebula",
                "loki",
                "valkyrie",
                // Villains
                "thanos",
                "ultron",
                "red_skull",
                "hela",
                "killmonger",
                "vulture",
                "mysterio",
                "green_goblin",
                "doc_ock",
                "venom",
                "carnage",
                "magneto",
                "mystique",
                "juggernaut",
                "sabretooth",
                // X-Men
                "wolverine",
                "cyclops",
                "jean_grey",
                "storm",
                "rogue",
                "beast",
                "nightcrawler",
                "colossus",
                "kitty_pryde",
                "iceman",
                "angel",
                "professor_x",
                "gambit",
                "jubilee",
                "psylocke",
                // Fantastic Four
                "mr_fantastic",
                "invisible_woman",
                "human_torch",
                "the_thing",
                // Street Level
                "daredevil",
                "punisher",
                "luke_cage",
                "iron_fist",
                "jessica_jones",
                "elektra",
                "blade",
                "moon_knight",
                "ghost_rider",
                "deadpool",
                // Cosmic
                "silver_surfer",
                "galactus",
                "nova",
                "captain_marvel",
                "ms_marvel",
                "adam_warlock",
                "quasar",
                "beta_ray_bill",
                // More heroes to trigger multiple flushes
                "she_hulk",
                "hawkgirl",
                "aquaman",
                "flash",
                "green_lantern",
                "cyborg",
                "martian_manhunter",
                "wonder_woman",
                "superman",
                "batman",
                "nightwing",
                "robin",
                "batgirl",
                "supergirl",
                "shazam",
                "plastic_man",
                "atom",
                "firestorm",
                "booster_gold",
                "blue_beetle",
            ];

            // Create 10KB zero'd out value
            let size = match i {
                0 => 10_000,
                1 => 100_000,
                2 => 500_000,
                3 => 1_000_000,
                _ => 2_000_000,
            };
            println!("batch {i} is of size: {size}");
            let power_data = vec![0u8; size];

            println!("Starting to insert {} entries via gRPC...", test_keys.len());

            for (idx, hero) in test_keys.iter().enumerate() {
                let request = Request::new(PutRequest {
                    key: hero.as_bytes().to_vec(),
                    value: power_data.clone(),
                    metadata: std::collections::HashMap::new(),
                });

                match client.put(request).await {
                    Ok(response) => {
                        if idx % 10 == 0 {
                            println!("Inserted {} entries so far", idx);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to insert '{}': {}", hero, e);
                    }
                }
            }

            /*
            client
                .delete(Request::new(DeleteRequest {
                    key: "drax".as_bytes().to_vec(),
                }))
                .await?;
            */

            println!("\n=== Test Data Generation Complete ===");
            println!("Total keys inserted: {}", test_keys.len());
            println!("batch {} completed sleeping for 7 seconds", i);

            thread::sleep(Duration::from_secs(5));
        }
        Ok(())
    }
}
