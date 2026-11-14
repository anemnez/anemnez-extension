//! Calling an Anemnez extension from its own descriptor, one way for every bridge (Substrate's Lua
//! mount, Agora's MCP face): JSON arguments to the request's exact bytes, the call over a raw-bytes
//! codec, and the reply's bytes back to JSON (CONTRACT.md `#every-bridge-encodes-a-call-alike`).
//!
//! Two rules (D, 2026-11-07):
//!
//! - **Strict on the way in.** A key the request message has no field for, or a value of the wrong
//!   type, is refused by name (`args-mismatch`), never dropped: an agent's typo (`repoo`) must not
//!   quietly become a call without that argument, which for a write is a different act.
//! - **The bytes it encodes are the bytes it sends.** [`Calls::encode`] answers the bytes, and the
//!   caller hands those same bytes to the principal token's body digest and to [`unary`], never
//!   re-encoding, so the digest is over exactly what the product receives.

pub mod events;

use bytes::{Buf, BufMut};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MethodDescriptor, SerializeOptions};

/// Why a call could not be built or read, each by name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallRefusal {
    /// The descriptor is not a `FileDescriptorSet` these types can read.
    Malformed(String),
    /// No method at that path in the descriptor.
    UnknownMethod(String),
    /// The arguments are not the request message: an unknown key, a wrong type, not an object.
    ArgsMismatch(String),
    /// The reply bytes are not the method's reply message.
    ReplyMismatch(String),
}

impl CallRefusal {
    /// The refusal's name.
    pub fn name(&self) -> &'static str {
        match self {
            CallRefusal::Malformed(_) => "malformed",
            CallRefusal::UnknownMethod(_) => "unknown-method",
            CallRefusal::ArgsMismatch(_) => "args-mismatch",
            CallRefusal::ReplyMismatch(_) => "reply-mismatch",
        }
    }
}

impl std::fmt::Display for CallRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self {
            CallRefusal::Malformed(why)
            | CallRefusal::UnknownMethod(why)
            | CallRefusal::ArgsMismatch(why)
            | CallRefusal::ReplyMismatch(why) => why,
        };
        write!(f, "{}: {why}", self.name())
    }
}

impl std::error::Error for CallRefusal {}

/// An extension's methods, read from the descriptor it serves (`Describe/Get`).
#[derive(Clone)]
pub struct Calls {
    pool: DescriptorPool,
}

impl Calls {
    /// The methods of an encoded `FileDescriptorSet`.
    ///
    /// # Errors
    /// [`CallRefusal::Malformed`] when it can't be read.
    pub fn new(file_descriptor_set: &[u8]) -> Result<Calls, CallRefusal> {
        DescriptorPool::decode(file_descriptor_set)
            .map(|pool| Calls { pool })
            .map_err(|e| CallRefusal::Malformed(e.to_string()))
    }

    /// The method at `path` (`/<package>.<Service>/<Method>`).
    ///
    /// # Errors
    /// [`CallRefusal::UnknownMethod`].
    pub fn method(&self, path: &str) -> Result<MethodDescriptor, CallRefusal> {
        let unknown = || CallRefusal::UnknownMethod(format!("no method {path} in the descriptor"));
        let (service, method) = path.strip_prefix('/').and_then(|p| p.split_once('/')).ok_or_else(unknown)?;
        self.pool
            .get_service_by_name(service)
            .and_then(|s| s.methods().find(|m| m.name() == method))
            .ok_or_else(unknown)
    }

    /// `args_json` (a JSON object of the request's fields, proto3 JSON mapping) as the request's bytes:
    /// the bytes to digest for a principal token AND to send, never re-encoded in between.
    ///
    /// # Errors
    /// [`CallRefusal::UnknownMethod`], or [`CallRefusal::ArgsMismatch`] for an unknown key or a value of
    /// the wrong type, never a silently dropped one.
    pub fn encode(&self, path: &str, args_json: &str) -> Result<Vec<u8>, CallRefusal> {
        let method = self.method(path)?;
        let mut json = serde_json::Deserializer::from_str(args_json);
        let message = DynamicMessage::deserialize(method.input(), &mut json)
            .map_err(|e| CallRefusal::ArgsMismatch(format!("{} for {path}", e)))?;
        json.end().map_err(|e| CallRefusal::ArgsMismatch(e.to_string()))?;
        Ok(message.encode_to_vec())
    }

    /// The reply's bytes as JSON, with the proto's own field names and every field present, so an agent
    /// reads `next: ""` rather than a missing key.
    ///
    /// # Errors
    /// [`CallRefusal::UnknownMethod`] or [`CallRefusal::ReplyMismatch`].
    pub fn decode(&self, path: &str, reply: &[u8]) -> Result<serde_json::Value, CallRefusal> {
        let method = self.method(path)?;
        let message =
            DynamicMessage::decode(method.output(), reply).map_err(|e| CallRefusal::ReplyMismatch(e.to_string()))?;
        let options = SerializeOptions::new().use_proto_field_name(true).skip_default_fields(false);
        message
            .serialize_with_options(serde_json::value::Serializer, &options)
            .map_err(|e| CallRefusal::ReplyMismatch(e.to_string()))
    }
}

/// A unary call to `path` with `request`, exactly the bytes [`Calls::encode`] answered, and `metadata`
/// (the principal token, when minted), answering the reply's bytes.
///
/// # Errors
/// The product's status, or the transport's (boxed: a `Status` is large).
pub async fn unary(
    channel: tonic::transport::Channel,
    path: &str,
    request: Vec<u8>,
    metadata: tonic::metadata::MetadataMap,
) -> Result<Vec<u8>, Box<tonic::Status>> {
    let path: http::uri::PathAndQuery =
        path.parse().map_err(|_| Box::new(tonic::Status::invalid_argument(format!("{path} is not a method path"))))?;
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.map_err(|e| Box::new(tonic::Status::unavailable(e.to_string())))?;
    let mut call = tonic::Request::new(request);
    *call.metadata_mut() = metadata;
    grpc.unary(call, path, Raw).await.map(tonic::Response::into_inner).map_err(Box::new)
}

/// A codec that sends and receives a message's bytes untouched.
#[derive(Debug, Default, Clone, Copy)]
pub struct Raw;

impl tonic::codec::Codec for Raw {
    type Encode = Vec<u8>;
    type Decode = Vec<u8>;
    type Encoder = Raw;
    type Decoder = Raw;

    fn encoder(&mut self) -> Raw {
        Raw
    }

    fn decoder(&mut self) -> Raw {
        Raw
    }
}

impl tonic::codec::Encoder for Raw {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn encode(&mut self, item: Vec<u8>, dst: &mut tonic::codec::EncodeBuf<'_>) -> Result<(), tonic::Status> {
        dst.put_slice(&item);
        Ok(())
    }
}

impl tonic::codec::Decoder for Raw {
    type Item = Vec<u8>;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut tonic::codec::DecodeBuf<'_>) -> Result<Option<Vec<u8>>, tonic::Status> {
        let n = src.remaining();
        Ok(Some(src.copy_to_bytes(n).to_vec()))
    }
}

#[cfg(test)]
mod client_tests;
