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
        if req.key.len() == 0 {
            warn!("recieved Put request with empty key");
            return Err(Status::invalid_argument("key argument cannot be empty"));
        }
        if req.value.len() == 0 {
            warn!("recieved Put request with empty value");
            return Err(Status::invalid_argument("value argument cannot be empty"));
        }

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
        info!("Delete request: {:?}", request);
        let req = request.into_inner();

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
        info!("Get request: {:?}", request);
        let req = request.into_inner();

        // lock the database for reading
        let db = self
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

// helper function to start the grpc server
pub async fn run_server(
    db: Arc<Mutex<Memtable>>,
    addr: std::net::SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let lsm_db = MyLsmDb::new(db);
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
            File::create("app.log")?,
        ),
    ])?;

    info!("lsm-db grpc server listening on {}", addr);

    // build and run the server with our service implementation
    Server::builder()
        .add_service(LsmdbServer::new(lsm_db))
        .serve(addr)
        .await?;

    Ok(())
}
