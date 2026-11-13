//! The connect-time check of an extension's descriptor (CONTRACT.md
//! `#every-bridge-refuses-a-descriptor-alike`): one pure function that every bridge (Substrate's
//! Lua mount, Agora's MCP face) and the conformance proof calls, so no bridge admits what another
//! refuses (B, D, H, 2026-11-07).
//!
//! The descriptor is the extension's `FileDescriptorSet`, as `Describe/Get` serves it. Its methods'
//! `anemnez_method` options (field 51701) are protobuf extensions, which `prost_types` drops on
//! decode, so this reads them from each method's raw option bytes itself. A method with no anemnez
//! options is not offered (nothing gets a face by omission); every offered method is checked, and
//! EVERY refusal is answered, not the first.

use prost::Message;
use prost_types::{DescriptorProto, FileDescriptorProto, FileDescriptorSet};

use crate::options::{
    Callers, Effect, MethodOptions, Permission, Presence, ResultShape, Route, RouteKey, Session, check_route_key,
};

/// The extension field number of `anemnez_method` on `google.protobuf.MethodOptions`.
pub const METHOD_OPTIONS_FIELD: u32 = 51701;

/// The extension field number of `anemnez_service` on `google.protobuf.ServiceOptions`.
pub const SERVICE_OPTIONS_FIELD: u32 = 51702;

/// One offered method, as the descriptor declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Method {
    /// `/<package>.<Service>/<Method>`, the path a call and a token name.
    pub path: String,
    pub options: MethodOptions,
    /// For a keyed route, the input field its key is read from.
    pub route_key: Option<RouteKey>,
    pub client_streaming: bool,
    pub server_streaming: bool,
    /// The request message's top-level fields, in declaration order: what a bridge builds the op's
    /// parameters from (C's LuaTypeInfo, B's MethodSpec), so no bridge re-parses the descriptor.
    pub input: Vec<Field>,
    /// The request message's full name (`.pkg.Message`).
    pub input_type: String,
    /// The reply message's full name.
    pub output_type: String,
}

/// One top-level field of a request message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Field {
    pub name: String,
    pub number: i32,
    pub kind: FieldKind,
    /// `repeated`: a list of `kind`.
    pub repeated: bool,
    /// A proto3 `optional` field, whose presence is tracked.
    pub optional: bool,
}

/// A field's type: a scalar, or the full name of an enum or message (`.pkg.Name`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldKind {
    Double,
    Float,
    Int64,
    Uint64,
    Int32,
    Uint32,
    Sint32,
    Sint64,
    Fixed32,
    Fixed64,
    Sfixed32,
    Sfixed64,
    Bool,
    String,
    Bytes,
    Enum(String),
    Message(String),
}

fn fields_of(message: Option<&DescriptorProto>) -> Vec<Field> {
    use prost_types::field_descriptor_proto::{Label, Type};
    let Some(message) = message else {
        return Vec::new();
    };
    message
        .field
        .iter()
        .map(|f| Field {
            name: f.name().to_owned(),
            number: f.number(),
            kind: match f.r#type() {
                Type::Double => FieldKind::Double,
                Type::Float => FieldKind::Float,
                Type::Int64 => FieldKind::Int64,
                Type::Uint64 => FieldKind::Uint64,
                Type::Int32 => FieldKind::Int32,
                Type::Uint32 => FieldKind::Uint32,
                Type::Sint32 => FieldKind::Sint32,
                Type::Sint64 => FieldKind::Sint64,
                Type::Fixed32 => FieldKind::Fixed32,
                Type::Fixed64 => FieldKind::Fixed64,
                Type::Sfixed32 => FieldKind::Sfixed32,
                Type::Sfixed64 => FieldKind::Sfixed64,
                Type::Bool => FieldKind::Bool,
                Type::String => FieldKind::String,
                Type::Bytes => FieldKind::Bytes,
                Type::Enum => FieldKind::Enum(f.type_name().to_owned()),
                Type::Message | Type::Group => FieldKind::Message(f.type_name().to_owned()),
            },
            repeated: f.label() == Label::Repeated,
            optional: f.proto3_optional(),
        })
        .collect()
}

/// Why a descriptor is refused at connect. Each names the method's path. `#[non_exhaustive]`, so a
/// new refusal is a minor release: a bridge reports one by [`DescriptorRefusal::name`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DescriptorRefusal {
    /// The bytes are not a `FileDescriptorSet`.
    Malformed(String),
    /// `callers` is not stated: nothing gets an agent face by omission.
    CallersMissing(String),
    /// `permission` is not stated: every method states one.
    PermissionMissing(String),
    /// A `person_act` method says `allow`.
    PersonActAllowed(String),
    /// No `result` shape: a collection must be a cursor or bounded, never unbounded.
    ResultMissing(String),
    /// No `effect`: whether it changes anything.
    EffectMissing(String),
    /// No `presence`.
    PresenceMissing(String),
    /// An empty `description`: it is written for the agent that calls the method.
    DescriptionMissing(String),
    /// A keyed route whose key is not a top-level, non-repeated string field of the input.
    RouteKey(String, crate::options::RouteKeyRefusal),
    /// A collection (an output with a repeated field) declared `one`: it must be a cursor or bounded.
    UnboundedCollection(String),
    /// The method's input type is not in the descriptor.
    InputUnknown(String),
    /// The anemnez options are not a valid `MethodOptions`.
    OptionsMalformed(String),
    /// An example's `args_json` is not a JSON object of the request's top-level fields, and why.
    ExampleMismatch(String, String),
    /// No service in the descriptor carries `anemnez_service`: it says nothing of what it serves.
    ServiceOptionsMissing,
    /// Two services carry `anemnez_service` (both named): a descriptor serves one contract.
    ServiceOptionsTwice(String, String),
    /// The service's `anemnez_service` is not a valid `ServiceOptions` (the service named).
    ServiceOptionsMalformed(String),
    /// The service names no contract (the service named).
    ContractMissing(String),
    /// The service states no version (the service named).
    VersionMissing(String),
    /// The service states no cursor scope (the service named).
    CursorScopeMissing(String),
}

impl DescriptorRefusal {
    /// The refusal's name, for a bridge's error.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::CallersMissing(_) => "callers-missing",
            Self::PermissionMissing(_) => "permission-missing",
            Self::PersonActAllowed(_) => "person-act-allowed",
            Self::ResultMissing(_) => "result-missing",
            Self::EffectMissing(_) => "effect-missing",
            Self::PresenceMissing(_) => "presence-missing",
            Self::DescriptionMissing(_) => "description-missing",
            Self::RouteKey(..) => "route-key",
            Self::UnboundedCollection(_) => "unbounded-collection",
            Self::InputUnknown(_) => "input-unknown",
            Self::OptionsMalformed(_) => "options-malformed",
            Self::ExampleMismatch(..) => "example-mismatch",
            Self::ServiceOptionsMissing => "service-options-missing",
            Self::ServiceOptionsTwice(..) => "service-options-twice",
            Self::ServiceOptionsMalformed(_) => "service-options-malformed",
            Self::ContractMissing(_) => "contract-missing",
            Self::VersionMissing(_) => "version-missing",
            Self::CursorScopeMissing(_) => "cursor-scope-missing",
        }
    }
}

impl std::fmt::Display for DescriptorRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed descriptor: {why}"),
            Self::RouteKey(path, why) => write!(f, "{path}: {why}"),
            Self::ExampleMismatch(path, why) => write!(f, "{path}: example-mismatch: {why}"),
            Self::ServiceOptionsMissing => f.write_str(self.name()),
            Self::ServiceOptionsTwice(first, second) => {
                write!(f, "{}: {first} and {second}", self.name())
            }
            Self::CallersMissing(p)
            | Self::PermissionMissing(p)
            | Self::PersonActAllowed(p)
            | Self::ResultMissing(p)
            | Self::EffectMissing(p)
            | Self::PresenceMissing(p)
            | Self::DescriptionMissing(p)
            | Self::UnboundedCollection(p)
            | Self::InputUnknown(p)
            | Self::OptionsMalformed(p)
            | Self::ServiceOptionsMalformed(p)
            | Self::ContractMissing(p)
            | Self::VersionMissing(p)
            | Self::CursorScopeMissing(p) => write!(f, "{p}: {}", self.name()),
        }
    }
}

impl std::error::Error for DescriptorRefusal {}

/// Check an encoded `FileDescriptorSet`: every offered method, with every refusal.
///
/// # Errors
/// Every [`DescriptorRefusal`] found, in descriptor order.
pub fn check_descriptor(file_descriptor_set: &[u8]) -> Result<Vec<Method>, Vec<DescriptorRefusal>> {
    let set = FileDescriptorSet::decode(file_descriptor_set)
        .map_err(|e| vec![DescriptorRefusal::Malformed(e.to_string())])?;
    let raw_files = repeated(file_descriptor_set, 1).map_err(|why| vec![DescriptorRefusal::Malformed(why.into())])?;
    let mut methods = Vec::new();
    let mut refused = Vec::new();
    if raw_files.len() != set.file.len() {
        return Err(vec![files_mismatch(set.file.len(), raw_files.len())]);
    }
    for (file, raw_file) in set.file.iter().zip(raw_files) {
        for (service, raw_service) in services_of(file, raw_file).map_err(|r| vec![r])? {
            let raw_methods =
                repeated(raw_service, 2).ok().filter(|raw| raw.len() == service.method.len()).ok_or_else(|| {
                    vec![DescriptorRefusal::Malformed(format!(
                        "{}: the methods of {} don't read raw",
                        file.name(),
                        service.name()
                    ))]
                })?;
            for (method, raw_method) in service.method.iter().zip(raw_methods) {
                let path = format!("/{}{}{}/{}", file.package(), dot(file.package()), service.name(), method.name());
                let bytes = match method_options(raw_method) {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => continue,
                    Err(_) => {
                        refused.push(DescriptorRefusal::OptionsMalformed(path));
                        continue;
                    }
                };
                let input = find_message(&set, method.input_type());
                let output = find_message(&set, method.output_type());
                match check_method(&path, &bytes, input, output) {
                    Ok((options, route_key)) => methods.push(Method {
                        path,
                        options,
                        route_key,
                        client_streaming: method.client_streaming(),
                        server_streaming: method.server_streaming(),
                        input: fields_of(input),
                        input_type: method.input_type().to_owned(),
                        output_type: method.output_type().to_owned(),
                    }),
                    Err(mut found) => refused.append(&mut found),
                }
            }
        }
    }
    if refused.is_empty() { Ok(methods) } else { Err(refused) }
}

fn dot(package: &str) -> &'static str {
    if package.is_empty() { "" } else { "." }
}

fn check_method(
    path: &str,
    bytes: &[u8],
    input: Option<&DescriptorProto>,
    output: Option<&DescriptorProto>,
) -> Result<(MethodOptions, Option<RouteKey>), Vec<DescriptorRefusal>> {
    let p = || path.to_owned();
    let raw = wire::MethodOptions::decode(bytes).map_err(|_| vec![DescriptorRefusal::OptionsMalformed(p())])?;
    let mut refused = Vec::new();
    let Parsed { callers, permission, effect, presence, session, result, route } = parse(&raw);
    if callers.is_none() {
        refused.push(DescriptorRefusal::CallersMissing(p()));
    }
    if permission.is_none() {
        refused.push(DescriptorRefusal::PermissionMissing(p()));
    }
    if raw.person_act && permission == Some(Permission::Allow) {
        refused.push(DescriptorRefusal::PersonActAllowed(p()));
    }
    if result.is_none() {
        refused.push(DescriptorRefusal::ResultMissing(p()));
    }
    if effect.is_none() {
        refused.push(DescriptorRefusal::EffectMissing(p()));
    }
    if presence.is_none() {
        refused.push(DescriptorRefusal::PresenceMissing(p()));
    }
    let collection = output
        .is_some_and(|o| o.field.iter().any(|f| f.label() == prost_types::field_descriptor_proto::Label::Repeated));
    if collection && result == Some(ResultShape::One) {
        refused.push(DescriptorRefusal::UnboundedCollection(p()));
    }
    if raw.description.trim().is_empty() {
        refused.push(DescriptorRefusal::DescriptionMissing(p()));
    }
    for example in &raw.examples {
        if let Err(why) = example_fits(&example.args_json, input) {
            refused.push(DescriptorRefusal::ExampleMismatch(p(), why));
        }
    }
    let mut route_key = None;
    if let Route::Keyed { key, .. } = &route {
        match input {
            None => refused.push(DescriptorRefusal::InputUnknown(p())),
            Some(input) => match check_route_key(input, key) {
                Ok(found) => route_key = Some(found),
                Err(why) => refused.push(DescriptorRefusal::RouteKey(p(), why)),
            },
        }
    }
    match (callers, permission, result, effect, presence) {
        (Some(callers), Some(permission), Some(result), Some(effect), Some(presence)) if refused.is_empty() => Ok((
            MethodOptions {
                description: raw.description,
                result,
                effect,
                route,
                person_act: raw.person_act,
                presence,
                permission,
                callers,
                session,
                blocks: raw.blocks,
                runs_to_completion: raw.runs_to_completion,
                examples: raw
                    .examples
                    .into_iter()
                    .map(|e| crate::options::Example::new(e.args_json, e.caption))
                    .collect(),
            },
            route_key,
        )),
        _ => Err(refused),
    }
}

/// The options' enums and oneofs as the kernel's types; `None` where a field is not stated.
struct Parsed {
    callers: Option<Callers>,
    permission: Option<Permission>,
    effect: Option<Effect>,
    presence: Option<Presence>,
    session: Option<Session>,
    result: Option<ResultShape>,
    route: Route,
}

fn parse(raw: &wire::MethodOptions) -> Parsed {
    let callers = match raw.callers {
        1 => Some(Callers::Agent),
        2 => Some(Callers::Product),
        3 => Some(Callers::Owner),
        _ => None,
    };
    let permission = match raw.permission {
        1 => Some(Permission::Allow),
        2 => Some(Permission::Ask),
        3 => Some(Permission::Deny),
        _ => None,
    };
    let effect = match raw.effect {
        1 => Some(Effect::Read),
        2 => Some(Effect::Write),
        3 => Some(Effect::Act),
        _ => None,
    };
    let presence = match raw.presence {
        1 => Some(Presence::Full),
        2 => Some(Presence::Minimal),
        3 => Some(Presence::Index),
        4 => Some(Presence::Hidden),
        _ => None,
    };
    let session = match raw.session {
        1 => Some(Session::Opens),
        2 => Some(Session::Within),
        3 => Some(Session::Closes),
        _ => None,
    };
    let result = match raw.result.clone().and_then(|r| r.shape) {
        Some(wire::Shape::Cursor(_)) => Some(ResultShape::Cursor),
        Some(wire::Shape::Bound(n)) => Some(ResultShape::Bound(n)),
        Some(wire::Shape::One(_)) => Some(ResultShape::One),
        None => None,
    };
    let route = match raw.route.clone().and_then(|r| r.to) {
        Some(wire::To::Leader(group)) => Route::Leader(group),
        Some(wire::To::Keyed(k)) => Route::Keyed { group: k.group, key: k.key },
        Some(wire::To::Any(_)) => Route::Any,
        Some(wire::To::LocalFirst(_)) | None => Route::LocalFirst,
    };
    Parsed { callers, permission, effect, presence, session, result, route }
}

/// Whether an example's `args_json` is a JSON object whose keys are all top-level fields of the
/// request `input` (when the descriptor holds it). The full type check is the bridge's.
fn example_fits(args_json: &str, input: Option<&DescriptorProto>) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(args_json).map_err(|e| format!("args_json is not JSON: {e}"))?;
    let object = value.as_object().ok_or("args_json is not a JSON object")?;
    if let Some(input) = input {
        for key in object.keys() {
            if !input.field.iter().any(|f| f.name() == key) {
                return Err(format!("'{key}' is not a field of {}", input.name()));
            }
        }
    }
    Ok(())
}

/// The message a fully qualified type name (`.pkg.Outer.Inner`) names, if the set holds it.
fn find_message<'a>(set: &'a FileDescriptorSet, type_name: &str) -> Option<&'a DescriptorProto> {
    let name = type_name.strip_prefix('.')?;
    set.file.iter().find_map(|file: &FileDescriptorProto| {
        let rest =
            if file.package().is_empty() { Some(name) } else { name.strip_prefix(file.package())?.strip_prefix('.') }?;
        let mut parts = rest.split('.');
        let outer = parts.next()?;
        let mut message = file.message_type.iter().find(|m| m.name() == outer)?;
        for part in parts {
            message = message.nested_type.iter().find(|m| m.name() == part)?;
        }
        Some(message)
    })
}

/// What the descriptor's one service declares of itself (`anemnez_service`): the contract it serves
/// and its version, read without a manifest. Every bridge reads it alike (Substrate's mount,
/// Agora's MCP face), so it lives beside [`check_descriptor`].
///
/// # Errors
/// [`DescriptorRefusal::Malformed`] for bytes that are not a `FileDescriptorSet`;
/// `service-options-missing` when no service carries the option, `service-options-twice` when two
/// do, and `service-options-malformed`, `contract-missing`, `version-missing` or
/// `cursor-scope-missing` for one that says too little.
pub fn service_of(file_descriptor_set: &[u8]) -> Result<crate::options::ServiceOptions, DescriptorRefusal> {
    let set =
        FileDescriptorSet::decode(file_descriptor_set).map_err(|e| DescriptorRefusal::Malformed(e.to_string()))?;
    let raw_files = repeated(file_descriptor_set, 1).map_err(|why| DescriptorRefusal::Malformed(why.into()))?;
    if raw_files.len() != set.file.len() {
        return Err(files_mismatch(set.file.len(), raw_files.len()));
    }
    let mut found: Option<(String, Vec<u8>)> = None;
    for (file, raw_file) in set.file.iter().zip(raw_files) {
        for (service, raw_service) in services_of(file, raw_file)? {
            let name = format!("{}{}{}", file.package(), dot(file.package()), service.name());
            let Some(bytes) = extension(raw_service, 3, SERVICE_OPTIONS_FIELD)
                .map_err(|_| DescriptorRefusal::ServiceOptionsMalformed(name.clone()))?
            else {
                continue;
            };
            if let Some((first, _)) = &found {
                return Err(DescriptorRefusal::ServiceOptionsTwice(first.clone(), name));
            }
            found = Some((name, bytes));
        }
    }
    let (name, bytes) = found.ok_or(DescriptorRefusal::ServiceOptionsMissing)?;
    let raw = wire::ServiceOptions::decode(bytes.as_slice())
        .map_err(|_| DescriptorRefusal::ServiceOptionsMalformed(name.clone()))?;
    if raw.contract.is_empty() {
        return Err(DescriptorRefusal::ContractMissing(name));
    }
    if raw.version.is_empty() {
        return Err(DescriptorRefusal::VersionMissing(name));
    }
    let cursor_scope = match raw.cursor_scope {
        1 => crate::options::CursorScope::Swarm,
        2 => crate::options::CursorScope::Instance,
        _ => return Err(DescriptorRefusal::CursorScopeMissing(name)),
    };
    Ok(crate::options::ServiceOptions {
        contract: raw.contract,
        version: raw.version,
        cursor_scope,
        parts_depth: raw.parts_depth,
        events_kept: raw.events_kept,
    })
}

fn files_mismatch(decoded: usize, raw: usize) -> DescriptorRefusal {
    DescriptorRefusal::Malformed(format!("{decoded} files decoded, {raw} read raw"))
}

/// A file's services, each with its raw bytes, or `malformed` naming the file when the raw list
/// can't be read or doesn't match the decoded one (D: never a silent truncation that would answer a
/// broken descriptor as service-options-missing).
fn services_of<'a>(
    file: &'a FileDescriptorProto,
    raw_file: &'a [u8],
) -> Result<Vec<(&'a prost_types::ServiceDescriptorProto, &'a [u8])>, DescriptorRefusal> {
    let malformed = |why: String| DescriptorRefusal::Malformed(format!("{}: {why}", file.name()));
    let raw = repeated(raw_file, 6).map_err(|why| malformed(why.to_string()))?;
    if raw.len() != file.service.len() {
        return Err(malformed(format!("{} services decoded, {} read raw", file.service.len(), raw.len())));
    }
    Ok(file.service.iter().zip(raw).collect())
}

/// The bytes of extension `number` in the options message at field `options_field` of a raw
/// descriptor message, every occurrence joined as protobuf merges them; `None` when it is absent,
/// and an error for bytes that can't be read, never an absence (a broken option is not a missing one).
fn extension(raw: &[u8], options_field: u32, number: u32) -> Result<Option<Vec<u8>>, &'static str> {
    let mut found: Option<Vec<u8>> = None;
    for block in repeated(raw, options_field)? {
        for bytes in repeated(block, number)? {
            found.get_or_insert_with(Vec::new).extend_from_slice(bytes);
        }
    }
    Ok(found)
}

/// The anemnez options of one raw `MethodDescriptorProto`: field 4 (`options`), and within it every
/// occurrence of field 51701, concatenated, which protobuf merges as one message.
fn method_options(raw_method: &[u8]) -> Result<Option<Vec<u8>>, &'static str> {
    extension(raw_method, 4, METHOD_OPTIONS_FIELD)
}

/// Every length-delimited occurrence of field `number` in a raw message. Every field read this way
/// is a message, so `number` with any other wire type is unreadable, never skipped.
fn repeated(message: &[u8], number: u32) -> Result<Vec<&[u8]>, &'static str> {
    let mut out = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let (key, after) = varint(rest)?;
        rest = after;
        let field = u32::try_from(key >> 3).map_err(|_| "field number too large")?;
        if field == number && key & 7 != 2 {
            return Err("a message field with another wire type");
        }
        match key & 7 {
            0 => rest = varint(rest)?.1,
            1 => rest = rest.get(8..).ok_or("short fixed64")?,
            2 => {
                let (len, after) = varint(rest)?;
                let len = usize::try_from(len).map_err(|_| "length too large")?;
                let bytes = after.get(..len).ok_or("short length-delimited field")?;
                if field == number {
                    out.push(bytes);
                }
                rest = &after[len..];
            }
            5 => rest = rest.get(4..).ok_or("short fixed32")?,
            _ => return Err("unsupported wire type"),
        }
    }
    Ok(out)
}

fn varint(bytes: &[u8]) -> Result<(u64, &[u8]), &'static str> {
    let mut value = 0u64;
    for (i, byte) in bytes.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value, &bytes[i + 1..]));
        }
    }
    Err("bad varint")
}

/// `anemnez.extension.v1.MethodOptions` as it travels, written by hand so the kernel needs no
/// build script; the serve crate's parity test holds it to the proto's generated code.
mod wire {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct MethodOptions {
        #[prost(string, tag = "1")]
        pub description: String,
        #[prost(message, optional, tag = "2")]
        pub result: Option<Result>,
        #[prost(int32, tag = "3")]
        pub effect: i32,
        #[prost(message, optional, tag = "4")]
        pub route: Option<Route>,
        #[prost(bool, tag = "5")]
        pub person_act: bool,
        #[prost(int32, tag = "6")]
        pub presence: i32,
        #[prost(int32, tag = "7")]
        pub permission: i32,
        #[prost(int32, tag = "8")]
        pub callers: i32,
        #[prost(int32, tag = "9")]
        pub session: i32,
        #[prost(bool, tag = "10")]
        pub blocks: bool,
        #[prost(bool, tag = "11")]
        pub runs_to_completion: bool,
        #[prost(message, repeated, tag = "13")]
        pub examples: Vec<WireExample>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Result {
        #[prost(oneof = "Shape", tags = "1, 2, 3")]
        pub shape: Option<Shape>,
    }

    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Shape {
        #[prost(message, tag = "1")]
        Cursor(Empty),
        #[prost(uint32, tag = "2")]
        Bound(u32),
        #[prost(message, tag = "3")]
        One(Empty),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Route {
        #[prost(oneof = "To", tags = "1, 2, 3, 4")]
        pub to: Option<To>,
    }

    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum To {
        #[prost(string, tag = "1")]
        Leader(String),
        #[prost(message, tag = "2")]
        LocalFirst(Empty),
        #[prost(message, tag = "3")]
        Any(Empty),
        #[prost(message, tag = "4")]
        Keyed(Leader),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Leader {
        #[prost(string, tag = "1")]
        pub group: String,
        #[prost(string, tag = "2")]
        pub key: String,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct WireExample {
        #[prost(string, tag = "1")]
        pub args_json: String,
        #[prost(string, tag = "2")]
        pub caption: String,
    }

    #[derive(Clone, Copy, PartialEq, prost::Message)]
    pub struct Empty {}

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ServiceOptions {
        #[prost(string, tag = "1")]
        pub contract: String,
        #[prost(string, tag = "2")]
        pub version: String,
        #[prost(int32, tag = "3")]
        pub cursor_scope: i32,
        #[prost(uint32, tag = "4")]
        pub parts_depth: u32,
        #[prost(uint32, tag = "5")]
        pub events_kept: u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{FieldDescriptorProto, MethodDescriptorProto, ServiceDescriptorProto};

    fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// A length-delimited field, as protoc writes an extension inside `MethodOptions`.
    fn ld(field: u32, bytes: &[u8]) -> Vec<u8> {
        let mut out = varint_bytes(u64::from(field) << 3 | 2);
        out.extend(varint_bytes(bytes.len() as u64));
        out.extend_from_slice(bytes);
        out
    }

    fn method(name: &str, input: &str, output: &str, options: Option<&wire::MethodOptions>) -> Vec<u8> {
        let mut bytes = MethodDescriptorProto {
            name: Some(name.into()),
            input_type: Some(input.into()),
            output_type: Some(output.into()),
            ..Default::default()
        }
        .encode_to_vec();
        if let Some(o) = options {
            bytes.extend(ld(4, &ld(METHOD_OPTIONS_FIELD, &o.encode_to_vec())));
        }
        bytes
    }

    fn message(name: &str, fields: &[(&str, i32, Type, Label)]) -> DescriptorProto {
        DescriptorProto {
            name: Some(name.into()),
            field: fields
                .iter()
                .map(|(n, number, ty, label)| FieldDescriptorProto {
                    name: Some((*n).into()),
                    number: Some(*number),
                    r#type: Some(*ty as i32),
                    label: Some(*label as i32),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn set(methods: Vec<Vec<u8>>) -> Vec<u8> {
        let mut service = ServiceDescriptorProto { name: Some("Herald".into()), ..Default::default() }.encode_to_vec();
        for m in methods {
            service.extend(ld(2, &m));
        }
        let mut file = FileDescriptorProto {
            name: Some("herald.proto".into()),
            package: Some("herald.v2".into()),
            message_type: vec![
                message(
                    "LandRequest",
                    &[("train", 1, Type::String, Label::Optional), ("repo", 3, Type::String, Label::Optional)],
                ),
                message("LandReply", &[]),
                message("QueueReply", &[("trains", 1, Type::String, Label::Repeated)]),
            ],
            ..Default::default()
        }
        .encode_to_vec();
        file.extend(ld(6, &service));
        ld(1, &file)
    }

    /// One file with one service per entry: its name and, when given, its `anemnez_service`.
    fn services(entries: &[(&str, Option<&wire::ServiceOptions>)]) -> Vec<u8> {
        let mut file = FileDescriptorProto {
            name: Some("herald.proto".into()),
            package: Some("herald.v2".into()),
            ..Default::default()
        }
        .encode_to_vec();
        for (name, options) in entries {
            let mut service =
                ServiceDescriptorProto { name: Some((*name).into()), ..Default::default() }.encode_to_vec();
            if let Some(o) = options {
                service.extend(ld(3, &ld(SERVICE_OPTIONS_FIELD, &o.encode_to_vec())));
            }
            file.extend(ld(6, &service));
        }
        ld(1, &file)
    }

    fn herald() -> wire::ServiceOptions {
        wire::ServiceOptions {
            contract: "herald".into(),
            version: "2.3.0".into(),
            cursor_scope: 2,
            parts_depth: 1,
            events_kept: 64,
        }
    }

    /// A bridge reads what a descriptor's one service declares of itself, with no manifest.
    #[test]
    fn service_of_reads_the_one_service_that_declares_itself() {
        let options = service_of(&services(&[("Admin", None), ("Herald", Some(&herald()))]))
            .expect("one service declares itself");
        assert_eq!(options.contract, "herald");
        assert_eq!(options.version, "2.3.0");
        assert_eq!(options.cursor_scope, crate::options::CursorScope::Instance);
        assert_eq!((options.parts_depth, options.events_kept), (1, 64));
    }

    /// D: a raw service list that doesn't match the decoded one is malformed, naming the file,
    /// never a silent truncation answered as service-options-missing.
    #[test]
    fn a_service_list_that_does_not_match_is_malformed() {
        let service = |name: &str| ServiceDescriptorProto { name: Some(name.into()), ..Default::default() };
        let file = FileDescriptorProto {
            name: Some("herald.proto".into()),
            service: vec![service("Herald"), service("Admin")],
            ..Default::default()
        };
        // The raw file carries one service where two were decoded: truncated.
        let raw = ld(6, &service("Herald").encode_to_vec());
        let refused = services_of(&file, &raw).expect_err("refused");
        assert_eq!(refused.name(), "malformed");
        assert!(refused.to_string().contains("herald.proto"), "{refused}");
    }

    /// A broken option is not a missing one: `anemnez_service` that can't be read (here, sent as a
    /// varint, which protobuf itself would skip as an unknown field) is malformed, naming the service.
    #[test]
    fn an_unreadable_service_option_is_malformed_not_missing() {
        let mut file = FileDescriptorProto {
            name: Some("herald.proto".into()),
            package: Some("herald.v2".into()),
            ..Default::default()
        }
        .encode_to_vec();
        let mut service = ServiceDescriptorProto { name: Some("Herald".into()), ..Default::default() }.encode_to_vec();
        let mut varint = Vec::new();
        prost::encoding::encode_key(SERVICE_OPTIONS_FIELD, prost::encoding::WireType::Varint, &mut varint);
        varint.push(1);
        service.extend(ld(3, &varint));
        file.extend(ld(6, &service));
        assert_eq!(
            service_of(&ld(1, &file)).expect_err("refused"),
            DescriptorRefusal::ServiceOptionsMalformed("herald.v2.Herald".into())
        );
    }

    /// The same on a method: an `anemnez_method` that can't be read is options-malformed by path,
    /// never 'not offered'.
    #[test]
    fn an_unreadable_method_option_is_malformed_not_unoffered() {
        let mut land = method("Land", ".herald.v2.LandRequest", ".herald.v2.LandReply", None);
        let mut varint = Vec::new();
        prost::encoding::encode_key(METHOD_OPTIONS_FIELD, prost::encoding::WireType::Varint, &mut varint);
        varint.push(1);
        land.extend(ld(4, &varint));
        let refused = check_descriptor(&set(vec![land])).expect_err("refused");
        assert_eq!(refused, vec![DescriptorRefusal::OptionsMalformed("/herald.v2.Herald/Land".into())]);
    }

    /// Every refusal names what is wrong: none declares, two declare, or one says too little.
    #[test]
    fn service_of_refuses_by_name() {
        let name = |r: Result<crate::options::ServiceOptions, DescriptorRefusal>| r.expect_err("refused").name();
        assert_eq!(name(service_of(&services(&[("Herald", None)]))), "service-options-missing");
        assert_eq!(
            service_of(&services(&[("Herald", Some(&herald())), ("Other", Some(&herald()))])).expect_err("refused"),
            DescriptorRefusal::ServiceOptionsTwice("herald.v2.Herald".into(), "herald.v2.Other".into())
        );
        let mut nameless = herald();
        nameless.contract.clear();
        assert_eq!(name(service_of(&services(&[("Herald", Some(&nameless))]))), "contract-missing");
        let mut unversioned = herald();
        unversioned.version.clear();
        assert_eq!(name(service_of(&services(&[("Herald", Some(&unversioned))]))), "version-missing");
        let mut scopeless = herald();
        scopeless.cursor_scope = 0;
        assert_eq!(name(service_of(&services(&[("Herald", Some(&scopeless))]))), "cursor-scope-missing");
        assert_eq!(name(service_of(b"not a descriptor")), "malformed");
    }

    fn good() -> wire::MethodOptions {
        wire::MethodOptions {
            description: "Land a train on its repo's main.".into(),
            result: Some(wire::Result { shape: Some(wire::Shape::One(wire::Empty {})) }),
            effect: 2,
            route: None,
            person_act: false,
            presence: 1,
            permission: 2,
            callers: 1,
            session: 0,
            blocks: false,
            runs_to_completion: false,
            examples: Vec::new(),
        }
    }

    /// every-bridge-refuses-a-descriptor-alike: a good descriptor answers every offered method with
    /// its options, a keyed route's field number, and skips a method that carries no anemnez options.
    #[test]
    fn a_good_descriptor_answers_every_offered_method() {
        let mut land = good();
        land.blocks = true;
        land.examples =
            vec![wire::WireExample { args_json: r#"{"repo": "agora"}"#.into(), caption: "Land agora".into() }];
        land.route = Some(wire::Route {
            to: Some(wire::To::Keyed(wire::Leader { group: "herald-repo".into(), key: "repo".into() })),
        });
        let mut queue = good();
        queue.effect = 1;
        queue.result = Some(wire::Result { shape: Some(wire::Shape::Cursor(wire::Empty {})) });
        queue.route = Some(wire::Route { to: Some(wire::To::Any(wire::Empty {})) });
        let bytes = set(vec![
            method("Land", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&land)),
            method("Queue", ".herald.v2.LandRequest", ".herald.v2.QueueReply", Some(&queue)),
            method("Internal", ".herald.v2.LandRequest", ".herald.v2.LandReply", None),
        ]);
        let methods = check_descriptor(&bytes).expect("admitted");
        let paths: Vec<_> = methods.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["/herald.v2.Herald/Land", "/herald.v2.Herald/Queue"], "Internal has no face");
        let input: Vec<_> =
            methods[0].input.iter().map(|f| (f.name.as_str(), f.number, f.kind.clone(), f.repeated)).collect();
        assert_eq!(
            input,
            [("train", 1, FieldKind::String, false), ("repo", 3, FieldKind::String, false)],
            "the request's fields come back, so no bridge re-parses the descriptor"
        );
        assert_eq!(methods[0].input_type, ".herald.v2.LandRequest");
        assert_eq!(methods[0].route_key, Some(RouteKey { name: "repo".into(), number: 3 }));
        assert_eq!(methods[0].options.route, Route::Keyed { group: "herald-repo".into(), key: "repo".into() });
        assert!(methods[0].options.blocks && !methods[0].options.runs_to_completion);
        assert_eq!(methods[0].options.examples, [crate::options::Example::new(r#"{"repo": "agora"}"#, "Land agora")]);
        assert_eq!(methods[1].options.result, ResultShape::Cursor);
        assert_eq!(methods[1].options.route, Route::Any);
    }

    /// Every refusal is answered, not the first, each naming its method.
    #[test]
    fn every_refusal_is_answered_by_name() {
        let bare = wire::MethodOptions::default();
        let mut person = good();
        person.person_act = true;
        person.permission = 1;
        let mut keyed = good();
        keyed.route = Some(wire::Route {
            to: Some(wire::To::Keyed(wire::Leader { group: "herald-repo".into(), key: "nope".into() })),
        });
        let mut lost = keyed.clone();
        lost.route =
            Some(wire::Route { to: Some(wire::To::Keyed(wire::Leader { group: "g".into(), key: "repo".into() })) });
        let bytes = set(vec![
            method("Bare", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&bare)),
            method("Person", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&person)),
            method("Keyed", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&keyed)),
            method("Queue", ".herald.v2.LandRequest", ".herald.v2.QueueReply", Some(&good())),
            method("Lost", ".herald.v2.Missing", ".herald.v2.LandReply", Some(&lost)),
        ]);
        let refused = check_descriptor(&bytes).expect_err("refused");
        let named: Vec<(&str, String)> = refused
            .iter()
            .map(|r| (r.name(), r.to_string().split(':').next().unwrap_or_default().to_owned()))
            .collect();
        let want = [
            ("callers-missing", "/herald.v2.Herald/Bare"),
            ("permission-missing", "/herald.v2.Herald/Bare"),
            ("result-missing", "/herald.v2.Herald/Bare"),
            ("effect-missing", "/herald.v2.Herald/Bare"),
            ("presence-missing", "/herald.v2.Herald/Bare"),
            ("description-missing", "/herald.v2.Herald/Bare"),
            ("person-act-allowed", "/herald.v2.Herald/Person"),
            ("route-key", "/herald.v2.Herald/Keyed"),
            ("unbounded-collection", "/herald.v2.Herald/Queue"),
            ("input-unknown", "/herald.v2.Herald/Lost"),
        ];
        let want: Vec<(&str, String)> = want.iter().map(|(n, p)| (*n, (*p).to_owned())).collect();
        assert_eq!(named, want);
    }

    #[test]
    fn bytes_that_are_not_a_descriptor_are_malformed() {
        let refused = check_descriptor(&[0xff, 0xff, 0xff]).expect_err("refused");
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].name(), "malformed");
    }

    /// A type is found by its full name wherever it sits in the file, nested types included.
    #[test]
    fn a_nested_or_later_input_type_is_found() {
        let mut outer = message("Outer", &[]);
        outer.nested_type.push(message("Inner", &[("repo", 7, Type::String, Label::Optional)]));
        let set = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                package: Some("p.v1".into()),
                message_type: vec![message("First", &[]), outer],
                ..Default::default()
            }],
        };
        assert_eq!(find_message(&set, ".p.v1.First").map(DescriptorProto::name), Some("First"));
        assert_eq!(find_message(&set, ".p.v1.Outer.Inner").map(DescriptorProto::name), Some("Inner"));
        assert_eq!(find_message(&set, ".p.v1.Inner"), None, "a nested type is not top-level");
        assert_eq!(find_message(&set, ".q.v1.First"), None, "another package");
    }

    /// An example must be a JSON object of the request's top-level fields, or the method is refused
    /// example-mismatch by name; the full type check is the bridge's.
    #[test]
    fn an_example_that_is_not_the_requests_fields_is_refused() {
        let example = |args: &str| wire::WireExample { args_json: args.into(), caption: String::new() };
        let mut unknown = good();
        unknown.examples = vec![example(r#"{"repo": "agora", "nope": 1}"#)];
        let mut not_object = good();
        not_object.examples = vec![example(r#"["agora"]"#)];
        let mut not_json = good();
        not_json.examples = vec![example("herald.land{repo = 'agora'}")];
        let bytes = set(vec![
            method("Unknown", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&unknown)),
            method("NotObject", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&not_object)),
            method("NotJson", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&not_json)),
        ]);
        let refused = check_descriptor(&bytes).expect_err("refused");
        let found: Vec<String> = refused.iter().map(ToString::to_string).collect();
        assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), ["example-mismatch"; 3]);
        assert!(found[0].contains("'nope' is not a field of LandRequest"), "{found:?}");
        assert!(found[1].contains("not a JSON object"), "{found:?}");
        assert!(found[2].contains("not JSON"), "{found:?}");
    }
}
