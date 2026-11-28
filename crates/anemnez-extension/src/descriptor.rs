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
    /// The reply's top-level `bytes` fields that say their media (`anemnez_field.media`), by name: what a bridge
    /// shows as an image (CONTRACT.md `#a-bytes-field-may-say-it-is-an-image`). Empty when none does.
    pub output_media: Vec<(String, crate::media::Media)>,
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
    /// A `map<K, V>` field: `repeated` of an entry message the descriptor marks `map_entry`, read
    /// from that option, never from the entry's name.
    pub map: bool,
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
            map: f.r#type() == Type::Message && is_map_entry(message, f.type_name()),
        })
        .collect()
}

/// Whether `type_name` is one of `message`'s nested types that the descriptor marks `map_entry`.
fn is_map_entry(message: &DescriptorProto, type_name: &str) -> bool {
    let entry = type_name.rsplit('.').next().unwrap_or_default();
    let parent = type_name.strip_suffix(entry).and_then(|p| p.strip_suffix('.')).unwrap_or_default();
    parent.rsplit('.').next() == Some(message.name())
        && message.nested_type.iter().any(|n| n.name() == entry && n.options.as_ref().is_some_and(|o| o.map_entry()))
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
    /// A route names an empty election group or residency fact (`leader`, `keyed`, `keyed_via`): a router would
    /// resolve a holder of "" (E m/22586). The second string names which part is empty.
    RouteUnnamed(String, &'static str),
    /// A collection (an output with a repeated field) declared `one`: it must be a cursor or bounded.
    UnboundedCollection(String),
    /// The method's input type is not in the descriptor.
    InputUnknown(String),
    /// The anemnez options are not a valid `MethodOptions`.
    OptionsMalformed(String),
    /// An example's `args_json` is not a JSON object of the request's top-level fields, and why.
    ExampleMismatch(String, String),
    /// `ask_within_secs` past the standard's cap ([`crate::options::ASK_WITHIN_CAP_SECS`]), with the value.
    AskBoundOverCap(String, u32),
    /// `bound_secs` of 0: no work takes no time, so it is said absent or stated (`bound-zero`).
    BoundZero(String),
    /// `bound_secs` past [`crate::options::BOUND_CAP_SECS`], with the value (`bound-over-cap`).
    BoundOverCap(String, u32),
    /// `ask_within_secs` not at least [`crate::options::ASK_MARGIN_SECS`] inside `bound_secs`, both stated
    /// (`ask-past-bound`): an Ask is held inside the method's work, so its bound is the inner one, strictly (rule 4).
    /// The ask's and the work's, said.
    AskPastBound(String, u32, u32),
    /// `bound_secs` too short to hold any Ask inside it, with no `ask_within_secs` stated (`bound-holds-no-ask`): the
    /// default would end at or past the work's bound. A method that never asks says 0.
    BoundHoldsNoAsk(String, u32),
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
    /// A field's `anemnez_field` is not a valid `FieldOptions` (the field named, `pkg.Message.field`).
    FieldOptionsMalformed(String),
    /// A field says a media not in [`crate::media::Media::ALL`] (the field named, and the media it said).
    MediaUnknown(String, String),
    /// A field that isn't `bytes` says a media (the field named): only bytes hold an image.
    MediaNotBytes(String),
    /// The service's guidance is longer than [`crate::options::GUIDANCE_MAX_CHARS`] (the service named, and its
    /// length in characters).
    GuidanceTooLong(String, usize),
    /// The guidance's first line, its summary, is longer than [`crate::options::GUIDANCE_SUMMARY_MAX_CHARS`] (the
    /// service named, and that line's length in characters).
    GuidanceSummaryTooLong(String, usize),
    /// Two methods are marked `orient` (both paths): a product says its "now" once.
    OrientTwice(String, String),
    /// An `orient` method whose effect isn't `read`: orienting changes nothing.
    OrientNotARead(String),
    /// An `orient` method whose request has fields: it takes no argument, so it renders the same for the same
    /// caller and state.
    OrientTakesInput(String),
    /// An `orient` method whose result isn't `bound: 1`: an orient answers one reply (K m/31487).
    OrientUnbounded(String),
    /// An `orient` method whose callers aren't `agent`: it orients an agent.
    OrientNotForAgents(String),
    /// An `orient` method that streams: it is one unary read.
    OrientStreams(String),
    /// A `same_by` repeat that names no top-level, non-repeated scalar field of the request (the path, and why).
    RepeatField(String, String),
    /// A `repeat` with no variant this build knows: a newer descriptor's, refused, never read as none.
    RepeatUnknown(String),
    /// A method's `causes` names a kind the registry doesn't declare (the path, and the kind); with the registry
    /// undeclared, every cause is (CONTRACT.md §1a).
    CauseUndeclared(String, String),
    /// A registry entry whose bytes aren't one hydramq message (its place in the registry, and why).
    EntryUnread(usize, String),
    /// A registry entry whose message isn't a `hydramq.registry.entry` (its place, and the type it said).
    EntryNotAnEntry(usize, String),
    /// A registry entry `registry::entry_schema` refuses (its kind, or its place, and what), or whose schema doesn't read.
    EntryInvalid(String, String),
    /// Two registry entries declare one kind (the kind).
    KindTwice(String),
    /// A deprecated entry that names no replacement: hydramq's `registry::check_entry` (its words).
    DeprecatedWithoutReplacement(String),
    /// An entry that writes its own `causes`: they are the methods' (option 17), filled by the serving face (G).
    CausesWritten(String),
    /// An entry's example body its kind's own schema refuses (the kind, the caption, and what).
    ExampleInvalid(String, String, String),
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
            Self::RouteUnnamed(..) => "route-unnamed",
            Self::UnboundedCollection(_) => "unbounded-collection",
            Self::InputUnknown(_) => "input-unknown",
            Self::OptionsMalformed(_) => "options-malformed",
            Self::ExampleMismatch(..) => "example-mismatch",
            Self::AskBoundOverCap(..) => "ask-bound-over-cap",
            Self::BoundZero(_) => "bound-zero",
            Self::BoundOverCap(..) => "bound-over-cap",
            Self::AskPastBound(..) => "ask-past-bound",
            Self::BoundHoldsNoAsk(..) => "bound-holds-no-ask",
            Self::ServiceOptionsMissing => "service-options-missing",
            Self::ServiceOptionsTwice(..) => "service-options-twice",
            Self::ServiceOptionsMalformed(_) => "service-options-malformed",
            Self::ContractMissing(_) => "contract-missing",
            Self::VersionMissing(_) => "version-missing",
            Self::CursorScopeMissing(_) => "cursor-scope-missing",
            Self::FieldOptionsMalformed(_) => "field-options-malformed",
            Self::MediaUnknown(..) => "media-unknown",
            Self::MediaNotBytes(_) => "media-not-bytes",
            Self::GuidanceTooLong(..) => "guidance-too-long",
            Self::GuidanceSummaryTooLong(..) => "guidance-summary-too-long",
            Self::OrientTwice(..) => "orient-twice",
            Self::OrientNotARead(_) => "orient-not-a-read",
            Self::OrientTakesInput(_) => "orient-takes-input",
            Self::OrientUnbounded(_) => "orient-unbounded",
            Self::OrientNotForAgents(_) => "orient-not-for-agents",
            Self::OrientStreams(_) => "orient-streams",
            Self::RepeatField(..) => "repeat-field",
            Self::RepeatUnknown(_) => "repeat-unknown",
            Self::CauseUndeclared(..) => "cause-undeclared",
            Self::EntryUnread(..) => "entry-unread",
            Self::EntryNotAnEntry(..) => "entry-not-an-entry",
            Self::EntryInvalid(..) => "entry-invalid",
            Self::KindTwice(_) => "kind-twice",
            Self::DeprecatedWithoutReplacement(_) => "deprecated-without-replacement",
            Self::CausesWritten(_) => "causes-written",
            Self::ExampleInvalid(..) => "example-invalid",
        }
    }
}

impl std::fmt::Display for DescriptorRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed descriptor: {why}"),
            Self::RouteKey(path, why) => write!(f, "{path}: {why}"),
            Self::RouteUnnamed(path, part) => write!(f, "{path}: the route's {part} is empty"),
            Self::ExampleMismatch(path, why) => write!(f, "{path}: example-mismatch: {why}"),
            Self::AskBoundOverCap(path, secs) => write!(
                f,
                "{path}: ask-bound-over-cap: ask_within_secs {secs} is past the cap of {}",
                crate::options::ASK_WITHIN_CAP_SECS
            ),
            Self::BoundZero(path) => {
                write!(f, "{path}: bound-zero: bound_secs 0 bounds no work; leave it absent, or state the bound")
            }
            Self::BoundOverCap(path, secs) => write!(
                f,
                "{path}: bound-over-cap: bound_secs {secs} is past the cap of {} (a job's {} less its {} margin)",
                crate::options::BOUND_CAP_SECS,
                crate::options::JOB_AT_MOST_SECS,
                crate::options::JOB_MARGIN_SECS
            ),
            Self::AskPastBound(path, ask, bound) => write!(
                f,
                "{path}: ask-past-bound: ask_within_secs {ask} doesn't end {}s inside bound_secs {bound}; an Ask is \
                 held inside the method's work, so its bound must be the shorter",
                crate::options::ASK_MARGIN_SECS
            ),
            Self::BoundHoldsNoAsk(path, bound) => write!(
                f,
                "{path}: bound-holds-no-ask: bound_secs {bound} leaves no Ask {}s inside it; state ask_within_secs 0 \
                 if the method never asks, or a longer bound",
                crate::options::ASK_MARGIN_SECS
            ),
            Self::ServiceOptionsMissing => f.write_str(self.name()),
            Self::MediaUnknown(path, media) => {
                write!(f, "{path}: media-unknown: {media:?} is not one of image/png, image/jpeg")
            }
            Self::ServiceOptionsTwice(first, second) | Self::OrientTwice(first, second) => {
                write!(f, "{}: {first} and {second}", self.name())
            }
            Self::GuidanceTooLong(service, chars) => write!(
                f,
                "{service}: guidance-too-long: {chars} characters, past {}",
                crate::options::GUIDANCE_MAX_CHARS
            ),
            Self::GuidanceSummaryTooLong(service, chars) => write!(
                f,
                "{service}: guidance-summary-too-long: its first line is {chars} characters, past {}",
                crate::options::GUIDANCE_SUMMARY_MAX_CHARS
            ),
            Self::OrientUnbounded(p) => write!(f, "{p}: orient-unbounded: an orient answers one reply: bound 1"),
            Self::RepeatField(p, why) => write!(f, "{p}: repeat-field: {why}"),
            Self::CauseUndeclared(p, kind) => {
                write!(f, "{p}: cause-undeclared: {kind:?} is not a kind the product's registry declares")
            }
            Self::EntryUnread(at, why) => write!(f, "registry entry {at}: entry-unread: {why}"),
            Self::EntryNotAnEntry(at, kind) => {
                write!(f, "registry entry {at}: entry-not-an-entry: its type is {kind:?}, not hydramq.registry.entry")
            }
            Self::EntryInvalid(kind, what) => write!(f, "{kind}: entry-invalid: {what}"),
            Self::KindTwice(kind) => write!(f, "{kind}: kind-twice: two entries declare it"),
            Self::DeprecatedWithoutReplacement(words) => f.write_str(words),
            Self::CausesWritten(kind) => write!(
                f,
                "{kind}: causes-written: an entry's causes are the methods' (option 17), filled by the serving face; \
                 drop them from the entry"
            ),
            Self::ExampleInvalid(kind, caption, what) => {
                write!(f, "{kind}: example-invalid: {caption:?} doesn't fit the kind's schema: {what}")
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
            | Self::CursorScopeMissing(p)
            | Self::FieldOptionsMalformed(p)
            | Self::MediaNotBytes(p)
            | Self::OrientNotARead(p)
            | Self::OrientTakesInput(p)
            | Self::OrientNotForAgents(p)
            | Self::OrientStreams(p)
            | Self::RepeatUnknown(p) => write!(f, "{p}: {}", self.name()),
        }
    }
}

impl std::error::Error for DescriptorRefusal {}

/// A method's gRPC path, `/<package>.<Service>/<Method>`: the one form every reader of a descriptor names a method by.
fn method_path(
    file: &prost_types::FileDescriptorProto,
    service: &prost_types::ServiceDescriptorProto,
    method: &prost_types::MethodDescriptorProto,
) -> String {
    format!("/{}{}{}/{}", file.package(), dot(file.package()), service.name(), method.name())
}

/// Every method of an encoded `FileDescriptorSet` whose request or response streams, by its gRPC path: what a drain must
/// tell End (M m/31062, D m/31056), read from the descriptor, the one place a method says it streams, so no product
/// lists its streams by hand (rule 2).
///
/// # Errors
/// [`DescriptorRefusal::Malformed`] for bytes that are not a `FileDescriptorSet`.
pub fn streaming_paths(file_descriptor_set: &[u8]) -> Result<Vec<String>, DescriptorRefusal> {
    let set =
        FileDescriptorSet::decode(file_descriptor_set).map_err(|e| DescriptorRefusal::Malformed(e.to_string()))?;
    let mut paths = Vec::new();
    for file in &set.file {
        for service in &file.service {
            for method in service.method.iter().filter(|m| m.client_streaming() || m.server_streaming()) {
                paths.push(method_path(file, service, method));
            }
        }
    }
    Ok(paths)
}

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
    // The first method marked orient, by path: a second is refused naming both (orient-twice).
    let mut orient: Option<String> = None;
    if raw_files.len() != set.file.len() {
        return Err(vec![files_mismatch(set.file.len(), raw_files.len())]);
    }
    let media = match crate::media::media_fields(file_descriptor_set) {
        Ok(found) => found,
        Err(mut found) => {
            refused.append(&mut found);
            Vec::new()
        }
    };
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
                let path = method_path(file, service, method);
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
                let streams = method.client_streaming() || method.server_streaming();
                match check_method(&path, &bytes, input, output) {
                    Ok((options, _)) if options.orient && streams => {
                        refused.push(DescriptorRefusal::OrientStreams(path));
                    }
                    Ok((options, _)) if options.orient && orient.is_some() => {
                        let first = orient.clone().unwrap_or_default();
                        refused.push(DescriptorRefusal::OrientTwice(first, path));
                    }
                    Ok((options, route_key)) => {
                        if options.orient {
                            orient = Some(path.clone());
                        }
                        methods.push(Method {
                            path,
                            options,
                            route_key,
                            client_streaming: method.client_streaming(),
                            server_streaming: method.server_streaming(),
                            input: fields_of(input),
                            input_type: method.input_type().to_owned(),
                            output_type: method.output_type().to_owned(),
                            output_media: media
                                .iter()
                                .filter(|f| f.message == method.output_type())
                                .map(|f| (f.field.clone(), f.media))
                                .collect(),
                        });
                    }
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

/// A method's two stated bounds, each against its cap: `ask_within_secs` past [`crate::options::ASK_WITHIN_CAP_SECS`],
/// and `bound_secs` at 0 or past [`crate::options::BOUND_CAP_SECS`], each by name.
fn bounds_refused(path: &str, raw: &wire::MethodOptions) -> Vec<DescriptorRefusal> {
    let mut refused = Vec::new();
    if let Some(secs) = raw.ask_within_secs.filter(|&s| s > crate::options::ASK_WITHIN_CAP_SECS) {
        refused.push(DescriptorRefusal::AskBoundOverCap(path.to_owned(), secs));
    }
    match raw.bound_secs {
        Some(0) => refused.push(DescriptorRefusal::BoundZero(path.to_owned())),
        Some(secs) if secs > crate::options::BOUND_CAP_SECS => {
            refused.push(DescriptorRefusal::BoundOverCap(path.to_owned(), secs));
        }
        _ => {}
    }
    // An Ask ends ASK_MARGIN_SECS inside the work's bound (D m/44391, E m/44479). A stated 0 never asks; an absent ask
    // reads its default inside the bound (MethodOptions::ask_within), so only a bound with no room for one is refused.
    if let Some(bound) = raw.bound_secs.filter(|&b| b > 0) {
        match raw.ask_within_secs {
            Some(0) => {}
            Some(ask) if ask.saturating_add(crate::options::ASK_MARGIN_SECS) > bound => {
                refused.push(DescriptorRefusal::AskPastBound(path.to_owned(), ask, bound));
            }
            Some(_) => {}
            None if bound <= crate::options::ASK_MARGIN_SECS => {
                refused.push(DescriptorRefusal::BoundHoldsNoAsk(path.to_owned(), bound));
            }
            None => {}
        }
    }
    refused
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
    refused.extend(bounds_refused(path, &raw));
    if raw.orient {
        if effect.is_some_and(|e| e != Effect::Read) {
            refused.push(DescriptorRefusal::OrientNotARead(p()));
        }
        if input.is_some_and(|i| !i.field.is_empty()) {
            refused.push(DescriptorRefusal::OrientTakesInput(p()));
        }
        if result.is_some() && result != Some(ResultShape::Bound(1)) {
            refused.push(DescriptorRefusal::OrientUnbounded(p()));
        }
        if callers.is_some_and(|c| c != crate::options::Callers::Agent) {
            refused.push(DescriptorRefusal::OrientNotForAgents(p()));
        }
    }
    let repeat = repeat_of(path, &raw, input, &mut refused);
    let mut route_key = None;
    let unnamed = match &route {
        Route::Leader(group) | Route::Keyed { group, .. } if group.is_empty() => Some("group"),
        Route::KeyedVia { group, .. } if group.is_empty() => Some("group"),
        Route::KeyedVia { via, .. } if via.is_empty() => Some("via"),
        _ => None,
    };
    if let Some(part) = unnamed {
        refused.push(DescriptorRefusal::RouteUnnamed(p(), part));
    }
    if let Some(key) = route.key() {
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
                ask_within_secs: raw.ask_within_secs,
                orient: raw.orient,
                bound_secs: raw.bound_secs,
                causes: raw.causes,
                repeat,
            },
            route_key,
        )),
        _ => Err(refused),
    }
}

/// A method's `repeat` as the kernel's type, each refusal it earns pushed onto `refused`: `same_by` names a top-level
/// scalar of the request (`repeat-field`), and a variant this build doesn't know is `repeat-unknown`, never none.
fn repeat_of(
    path: &str,
    raw: &wire::MethodOptions,
    input: Option<&DescriptorProto>,
    refused: &mut Vec<DescriptorRefusal>,
) -> crate::options::Repeat {
    let p = || path.to_owned();
    match raw.repeat.as_ref().map(|r| &r.how) {
        None | Some(Some(wire::How::None(_))) => crate::options::Repeat::None,
        Some(Some(wire::How::SameBy(by))) => match input.map(|i| crate::options::check_same_by(i, &by.field)) {
            Some(Ok(number)) => crate::options::Repeat::SameBy { field: by.field.clone(), number },
            Some(Err(why)) => {
                refused.push(DescriptorRefusal::RepeatField(p(), why));
                crate::options::Repeat::None
            }
            None => {
                refused.push(DescriptorRefusal::InputUnknown(p()));
                crate::options::Repeat::None
            }
        },
        // Present with no variant this build knows (prost drops an unknown oneof tag): a newer descriptor's, refused,
        // never read as none (rule 8).
        Some(None) => {
            refused.push(DescriptorRefusal::RepeatUnknown(p()));
            crate::options::Repeat::None
        }
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
    let permission = Permission::from_wire(raw.permission);
    let effect = match raw.effect {
        1 => Some(Effect::Read),
        2 => Some(Effect::Write),
        3 => Some(Effect::Act),
        _ => None,
    };
    let presence = Presence::from_wire(raw.presence);
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
    // A route present with no variant this build knows is a newer descriptor's: Unknown, never the default (prost
    // drops an unknown oneof tag, so a present Route with `to` unset is exactly that). No route at all is local-first.
    let route = match raw.route.clone().map(|r| r.to) {
        Some(Some(wire::To::Leader(group))) => Route::Leader(group),
        Some(Some(wire::To::Keyed(k))) => Route::Keyed { group: k.group, key: k.key },
        Some(Some(wire::To::KeyedVia(k))) => Route::KeyedVia { group: k.group, key: k.key, via: k.via },
        Some(Some(wire::To::Node(_))) => Route::Node,
        Some(Some(wire::To::Any(_))) => Route::Any,
        Some(Some(wire::To::LocalFirst(_))) | None => Route::LocalFirst,
        Some(None) => Route::Unknown,
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
    let chars = raw.guidance.chars().count();
    if chars > crate::options::GUIDANCE_MAX_CHARS {
        return Err(DescriptorRefusal::GuidanceTooLong(name, chars));
    }
    let summary = raw.guidance.lines().next().unwrap_or_default().chars().count();
    if summary > crate::options::GUIDANCE_SUMMARY_MAX_CHARS {
        return Err(DescriptorRefusal::GuidanceSummaryTooLong(name, summary));
    }
    Ok(crate::options::ServiceOptions {
        contract: raw.contract,
        version: raw.version,
        cursor_scope,
        parts_depth: raw.parts_depth,
        events_kept: raw.events_kept,
        guidance: raw.guidance,
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
pub(crate) fn extension(raw: &[u8], options_field: u32, number: u32) -> Result<Option<Vec<u8>>, &'static str> {
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
pub(crate) fn repeated(message: &[u8], number: u32) -> Result<Vec<&[u8]>, &'static str> {
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
        #[prost(uint32, optional, tag = "14")]
        pub ask_within_secs: Option<u32>,
        #[prost(bool, tag = "15")]
        pub orient: bool,
        #[prost(uint32, optional, tag = "16")]
        pub bound_secs: Option<u32>,
        #[prost(string, repeated, tag = "17")]
        pub causes: Vec<String>,
        #[prost(message, optional, tag = "18")]
        pub repeat: Option<Repeat>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Repeat {
        #[prost(oneof = "How", tags = "1, 2")]
        pub how: Option<How>,
    }

    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum How {
        #[prost(message, tag = "1")]
        None(Empty),
        #[prost(message, tag = "2")]
        SameBy(SameBy),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct SameBy {
        #[prost(string, tag = "1")]
        pub field: String,
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
        #[prost(oneof = "To", tags = "1, 2, 3, 4, 5, 6")]
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
        #[prost(message, tag = "5")]
        Node(Empty),
        #[prost(message, tag = "6")]
        KeyedVia(KeyedVia),
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct KeyedVia {
        #[prost(string, tag = "1")]
        pub group: String,
        #[prost(string, tag = "2")]
        pub key: String,
        #[prost(string, tag = "3")]
        pub via: String,
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
        #[prost(string, tag = "6")]
        pub guidance: String,
    }
}

#[cfg(test)]
#[path = "descriptor_tests.rs"]
mod tests;
