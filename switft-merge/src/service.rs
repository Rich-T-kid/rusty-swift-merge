use tonic::{transport::Server, Request, Response, Status};
use std::sync::{Arc, Mutex};
use crate::memtable::mem::{Memtable, TableEntry, TypeInfoMetadata, TrueTypes};
use std::collections::BTreeMap;

// import the generated rust code from proto
pub mod swiftmerge {
    tonic::include_proto!("swiftmerge.v01");
}

// pull in the server trait and message types from the generated code
use swiftmerge::lsmdb_server::{Lsmdb, LsmdbServer};
pub use swiftmerge::{
    DeleteRequest, GenericResponse, GetRequest, GetResponse, PutRequest, 
    ReadMetricsRequest, ReadMetricsResponse, WriteMetricsRequest, WriteMetricsResponse,
    SupportedMetadataTypes, TypeInfo
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
    async fn put(
        &self,
        request: Request<PutRequest>,
    ) -> Result<Response<GenericResponse>, Status> {
        let req = request.into_inner();
        
        // map grpc metadata to internal typeinfo metadata
        let mut internal_metadata = BTreeMap::new();
        for (key, meta) in req.metadata {
            // convert the proto enum to our internal rust enum
            let true_type = match SupportedMetadataTypes::try_from(meta.true_type) {
                Ok(SupportedMetadataTypes::SupportedMetadataTypesBool) => TrueTypes::Bool,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesRawByte) => TrueTypes::RawBytes,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesString) => TrueTypes::String,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesUint32) => TrueTypes::Uint32,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesUint64) => TrueTypes::Uint64,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesInt32) => TrueTypes::Int32,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesInt64) => TrueTypes::Int64,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesFloat32) => TrueTypes::Float32,
                Ok(SupportedMetadataTypes::SupportedMetadataTypesDouble) => TrueTypes::Double,
                _ => TrueTypes::Unspecified,
            };
            
            internal_metadata.insert(key, TypeInfoMetadata::new(meta.raw, true_type));
        }

        // package the value and metadata into a table entry
        let entry = TableEntry::new(req.value, Some(internal_metadata));
        
        // lock the database and perform the write operation
        let mut db = self.db.lock().map_err(|_| Status::internal("lock poisoned"))?;
        db.put(&req.key, entry).map_err(|e| Status::internal(format!("db error: {:?}", e)))?;

        Ok(Response::new(GenericResponse {}))
    }

    // handle delete requests to remove data using tombstones
    async fn delete(
        &self,
        request: Request<DeleteRequest>,
    ) -> Result<Response<GenericResponse>, Status> {
        let req = request.into_inner();
        
        // lock the database and write a tombstone for the key
        let mut db = self.db.lock().map_err(|_| Status::internal("lock poisoned"))?;
        db.delete(&req.key).map_err(|e| Status::internal(format!("db error: {:?}", e)))?;

        Ok(Response::new(GenericResponse {}))
    }

    // handle get requests to retrieve data
    async fn get(
        &self,
        request: Request<GetRequest>,
    ) -> Result<Response<GetResponse>, Status> {
        let req = request.into_inner();
        
        // lock the database for reading
        let db = self.db.lock().map_err(|_| Status::internal("lock poisoned"))?;
        
        // look up the key in the memtable
        match db.get(&req.key) {
            Ok(Some(entry)) => {
                // map internal metadata back to grpc typeinfo for the client
                let mut grpc_metadata = BTreeMap::new();
                if let Some(md) = &entry.meta_data {
                    for (key, meta) in md {
                        // convert our internal rust enum back to the proto enum
                        let grpc_type = match meta.true_type {
                            TrueTypes::Bool => SupportedMetadataTypes::SupportedMetadataTypesBool,
                            TrueTypes::RawBytes => SupportedMetadataTypes::SupportedMetadataTypesRawByte,
                            TrueTypes::String => SupportedMetadataTypes::SupportedMetadataTypesString,
                            TrueTypes::Uint32 => SupportedMetadataTypes::SupportedMetadataTypesUint32,
                            TrueTypes::Uint64 => SupportedMetadataTypes::SupportedMetadataTypesUint64,
                            TrueTypes::Int32 => SupportedMetadataTypes::SupportedMetadataTypesInt32,
                            TrueTypes::Int64 => SupportedMetadataTypes::SupportedMetadataTypesInt64,
                            TrueTypes::Float32 => SupportedMetadataTypes::SupportedMetadataTypesFloat32,
                            TrueTypes::Double => SupportedMetadataTypes::SupportedMetadataTypesDouble,
                            _ => SupportedMetadataTypes::SupportedMetadataTypesUnspecified,
                        };
                        
                        grpc_metadata.insert(key.clone(), TypeInfo {
                            raw: meta.raw.clone(),
                            true_type: grpc_type as i32,
                        });
                    }
                }

                // apply metadata filtering if requested by the client
                if let Some(filter) = req.filter {
                    if filter.use_ {
                        // keep only the keys specified in the filter
                        grpc_metadata.retain(|k, _| filter.metadata_keys.contains(k));
                    }
                }

                // return the value and metadata
                Ok(Response::new(GetResponse {
                    value: Some(entry.value.clone()),
                    metadata: grpc_metadata,
                }))
            },
            Ok(None) => {
                // return an empty response if the key was deleted (tombstone)
                Ok(Response::new(GetResponse {
                    value: None,
                    metadata: BTreeMap::new(),
                }))
            },
            Err(_) => {
                // return an empty response if the key was not found
                Ok(Response::new(GetResponse {
                    value: None,
                    metadata: BTreeMap::new(),
                }))
            }
        }
    }

    // metrics are not yet tracked by the backend storage
    async fn write_metrics(
        &self,
        _request: Request<WriteMetricsRequest>,
    ) -> Result<Response<WriteMetricsResponse>, Status> {
        Err(Status::unimplemented("metrics not yet implemented in backend"))
    }

    // metrics are not yet tracked by the backend storage
    async fn read_metrics(
        &self,
        _request: Request<ReadMetricsRequest>,
    ) -> Result<Response<ReadMetricsResponse>, Status> {
        Err(Status::unimplemented("metrics not yet implemented in backend"))
    }
}

// helper function to start the grpc server
pub async fn run_server(db: Arc<Mutex<Memtable>>, addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let lsm_db = MyLsmDb::new(db);

    println!("lsm-db grpc server listening on {}", addr);

    // build and run the server with our service implementation
    Server::builder()
        .add_service(LsmdbServer::new(lsm_db))
        .serve(addr)
        .await?;

    Ok(())
}
