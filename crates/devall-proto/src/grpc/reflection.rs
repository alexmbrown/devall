//! gRPC server reflection client: lists services and downloads their file descriptors (with
//! transitive dependencies). Tries `grpc.reflection.v1` first, then `v1alpha`.

use std::collections::{BTreeMap, BTreeSet};

use prost::Message;
use prost_types::FileDescriptorProto;
use protox::file::FileResolver;
use tokio::sync::mpsc;
use tonic::metadata::MetadataMap;
use tonic::transport::Channel;
use tonic::{Code, Status};

/// What the server told us.
#[derive(Debug, Clone)]
pub struct Reflected {
    /// Every service the server lists (including the reflection service itself).
    pub services: Vec<String>,
    pub files: Vec<FileDescriptorProto>,
    /// Services whose descriptors couldn't be fetched.
    pub warnings: Vec<String>,
}

fn add_files(files: &mut BTreeMap<String, FileDescriptorProto>, encoded: Vec<Vec<u8>>) -> Result<(), Status> {
    for bytes in encoded {
        let f = FileDescriptorProto::decode(bytes.as_slice()).map_err(|e| Status::internal(format!("invalid file descriptor from server: {e}")))?;
        files.entry(f.name().to_string()).or_insert(f);
    }
    Ok(())
}

/// Google's well-known types, for servers that don't serve them.
fn builtin(name: &str) -> Option<FileDescriptorProto> {
    protox::file::GoogleFileResolver::new().open_file(name).ok().map(|f| f.file_descriptor_proto().clone())
}

fn missing_deps(files: &BTreeMap<String, FileDescriptorProto>) -> BTreeSet<String> {
    files.values().flat_map(|f| f.dependency.iter()).filter(|d| !files.contains_key(*d)).cloned().collect()
}

macro_rules! reflect_impl {
    ($fn_name:ident, $pb:ident) => {
        async fn $fn_name(channel: Channel, metadata: MetadataMap) -> Result<Reflected, Status> {
            use tonic_reflection::pb::$pb::server_reflection_client::ServerReflectionClient;
            use tonic_reflection::pb::$pb::server_reflection_request::MessageRequest;
            use tonic_reflection::pb::$pb::server_reflection_response::MessageResponse;
            use tonic_reflection::pb::$pb::ServerReflectionRequest;

            let (tx, rx) = mpsc::unbounded_channel::<ServerReflectionRequest>();
            let ask = |r: MessageRequest| tx.send(ServerReflectionRequest { host: String::new(), message_request: Some(r) }).map_err(|_| Status::internal("reflection stream closed"));
            ask(MessageRequest::ListServices(String::new()))?;
            let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|m| (m, rx)) });
            let mut req = tonic::Request::new(stream);
            *req.metadata_mut() = metadata;
            let mut client = ServerReflectionClient::new(channel).max_decoding_message_size(64 * 1024 * 1024);
            let mut resp = client.server_reflection_info(req).await?.into_inner();

            async fn next(resp: &mut tonic::Streaming<tonic_reflection::pb::$pb::ServerReflectionResponse>) -> Result<MessageResponse, Status> {
                let m = resp.message().await?.ok_or_else(|| Status::internal("reflection stream ended early"))?;
                m.message_response.ok_or_else(|| Status::internal("empty reflection response"))
            }

            let services: Vec<String> = match next(&mut resp).await? {
                MessageResponse::ListServicesResponse(l) => l.service.into_iter().map(|s| s.name).collect(),
                MessageResponse::ErrorResponse(e) => return Err(Status::new(Code::from(e.error_code), e.error_message)),
                _ => return Err(Status::internal("unexpected reflection response to ListServices")),
            };

            let mut files: BTreeMap<String, FileDescriptorProto> = BTreeMap::new();
            let mut warnings = Vec::new();
            for svc in services.iter().filter(|s| !s.starts_with("grpc.reflection.")) {
                ask(MessageRequest::FileContainingSymbol(svc.clone()))?;
                match next(&mut resp).await? {
                    MessageResponse::FileDescriptorResponse(f) => add_files(&mut files, f.file_descriptor_proto)?,
                    MessageResponse::ErrorResponse(e) => warnings.push(format!("{svc}: {}", e.error_message)),
                    _ => warnings.push(format!("{svc}: unexpected reflection response")),
                }
            }
            loop {
                let missing = missing_deps(&files);
                if missing.is_empty() {
                    break;
                }
                for name in missing {
                    ask(MessageRequest::FileByFilename(name.clone()))?;
                    let why = match next(&mut resp).await? {
                        MessageResponse::FileDescriptorResponse(f) => {
                            add_files(&mut files, f.file_descriptor_proto)?;
                            "not returned by the server".to_string()
                        }
                        MessageResponse::ErrorResponse(e) => e.error_message,
                        _ => "unexpected reflection response".to_string(),
                    };
                    // Fall back to the bundled well-known types; anything else missing is fatal
                    // (and guarantees the loop terminates).
                    if !files.contains_key(&name) {
                        match builtin(&name) {
                            Some(f) => {
                                files.insert(name, f);
                            }
                            None => return Err(Status::not_found(format!("dependency {name}: {why}"))),
                        }
                    }
                }
            }
            drop(tx);
            Ok(Reflected { services, files: files.into_values().collect(), warnings })
        }
    };
}

reflect_impl!(fetch_v1, v1);
reflect_impl!(fetch_v1alpha, v1alpha);

/// Fetches services and descriptors, preferring reflection v1.
pub async fn fetch(channel: Channel, metadata: MetadataMap) -> Result<Reflected, Status> {
    match fetch_v1(channel.clone(), metadata.clone()).await {
        Ok(r) => Ok(r),
        Err(s) if s.code() == Code::Unimplemented => fetch_v1alpha(channel, metadata).await.map_err(|e| {
            if e.code() == Code::Unimplemented {
                Status::unimplemented("the server does not support gRPC reflection (v1 or v1alpha); add its .proto files instead")
            } else {
                e
            }
        }),
        Err(s) => Err(s),
    }
}
