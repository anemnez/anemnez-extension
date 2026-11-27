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
pub mod health;
#[cfg(test)]
mod test_socket;

pub use health::{HEALTH_PATH, HealthUnread, health};

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
            .map_err(|e| CallRefusal::ArgsMismatch(args_mismatch(&method.input(), args_json, &e, path)))?;
        json.end().map_err(|e| CallRefusal::ArgsMismatch(where_in(&e, path)))?;
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

/// Why `args_json` isn't `input`, naming a key and the kinds of value only, never a value (E m/41989): serde quotes a
/// mistyped value (`invalid type: string "…"`), and a token or a person's text passed into the wrong field would reach
/// the caller's transcript. A top-level key the message lacks, or whose JSON kind the field can never take, is named;
/// anything deeper is said by where it is (serde's line and column), never by what it holds.
fn args_mismatch(
    input: &prost_reflect::MessageDescriptor,
    args_json: &str,
    e: &serde_json::Error,
    path: &str,
) -> String {
    let message = input.full_name();
    let Ok(serde_json::Value::Object(args)) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return format!("the arguments aren't a JSON object of {message}'s fields, for {path} ({})", place(e));
    };
    for (key, value) in &args {
        let Some(field) = input.get_field_by_json_name(key).or_else(|| input.get_field_by_name(key)) else {
            return if field_shaped(key) {
                format!("{message} has no field `{key}`, for {path}")
            } else {
                // A key is the caller's input too: one that doesn't read as a field name may be a value put where a
                // key goes, so it is said by its length alone (D m/42079).
                format!("a key of {} chars isn't a field of {message}, for {path}", key.chars().count())
            };
        };
        if let Some(expected) = never_takes(&field, value) {
            return format!("`{key}` takes {expected}, not {}, for {path}", json_kind(value));
        }
    }
    format!("an argument doesn't fit {message}, for {path} ({})", place(e))
}

/// Whether `key` reads as a field name, so naming it can't repeat a value: at most 64 of `[A-Za-z0-9_]` (D m/42079).
fn field_shaped(key: &str) -> bool {
    !key.is_empty() && key.len() <= 64 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The JSON kind of `value`, never the value.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// What `field` takes, when the JSON kind of `value` is one proto3's JSON mapping never accepts for it; `None` when it
/// may (a 64-bit number as a string, an enum by name or number, a message whose JSON form is its own).
fn never_takes(field: &prost_reflect::FieldDescriptor, value: &serde_json::Value) -> Option<&'static str> {
    use prost_reflect::Kind;
    use serde_json::Value;
    if value.is_null() {
        return None;
    }
    if field.is_map() {
        return (!value.is_object()).then_some("an object");
    }
    if field.is_list() {
        return (!value.is_array()).then_some("an array");
    }
    match field.kind() {
        Kind::String | Kind::Bytes => (!value.is_string()).then_some("a string"),
        Kind::Bool => (!value.is_boolean()).then_some("a boolean"),
        Kind::Enum(_) => (!matches!(value, Value::String(_) | Value::Number(_))).then_some("an enum name or number"),
        Kind::Message(_) => None,
        _ => (!matches!(value, Value::String(_) | Value::Number(_))).then_some("a number"),
    }
}

/// Where serde stopped, never what it read.
fn place(e: &serde_json::Error) -> String {
    format!("line {}, column {}", e.line(), e.column())
}

/// A trailing-input refusal, by where it is.
fn where_in(e: &serde_json::Error, path: &str) -> String {
    format!("more than one JSON value, for {path} ({})", place(e))
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
