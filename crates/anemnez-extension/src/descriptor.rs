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
    Callers, Effect, MethodOptions, Permission, Presence, ResultShape, Route, RouteKey, Session,
    check_route_key,
};

/// The extension field number of `anemnez_method` on `google.protobuf.MethodOptions`.
pub const METHOD_OPTIONS_FIELD: u32 = 51701;

/// One offered method, as the descriptor declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    /// `/<package>.<Service>/<Method>`, the path a call and a token name.
    pub path: String,
    pub options: MethodOptions,
    /// For a keyed route, the input field its key is read from.
    pub route_key: Option<RouteKey>,
    pub client_streaming: bool,
    pub server_streaming: bool,
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
        }
    }
}

impl std::fmt::Display for DescriptorRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "malformed descriptor: {why}"),
            Self::RouteKey(path, why) => write!(f, "{path}: {why}"),
            Self::CallersMissing(p)
            | Self::PermissionMissing(p)
            | Self::PersonActAllowed(p)
            | Self::ResultMissing(p)
            | Self::EffectMissing(p)
            | Self::PresenceMissing(p)
            | Self::DescriptionMissing(p)
            | Self::UnboundedCollection(p)
            | Self::InputUnknown(p)
            | Self::OptionsMalformed(p) => write!(f, "{p}: {}", self.name()),
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
    let raw_files = repeated(file_descriptor_set, 1)
        .map_err(|why| vec![DescriptorRefusal::Malformed(why.into())])?;
    let mut methods = Vec::new();
    let mut refused = Vec::new();
    for (file, raw_file) in set.file.iter().zip(raw_files) {
        let raw_services = repeated(raw_file, 6).unwrap_or_default();
        for (service, raw_service) in file.service.iter().zip(raw_services) {
            let raw_methods = repeated(raw_service, 2).unwrap_or_default();
            for (method, raw_method) in service.method.iter().zip(raw_methods) {
                let path = format!(
                    "/{}{}{}/{}",
                    file.package(),
                    dot(file.package()),
                    service.name(),
                    method.name()
                );
                let Some(bytes) = method_options(raw_method) else {
                    continue;
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
                    }),
                    Err(mut found) => refused.append(&mut found),
                }
            }
        }
    }
    if refused.is_empty() {
        Ok(methods)
    } else {
        Err(refused)
    }
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
    let raw = wire::MethodOptions::decode(bytes)
        .map_err(|_| vec![DescriptorRefusal::OptionsMalformed(p())])?;
    let mut refused = Vec::new();
    let Parsed {
        callers,
        permission,
        effect,
        presence,
        session,
        result,
        route,
    } = parse(&raw);
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
    let collection = output.is_some_and(|o| {
        o.field
            .iter()
            .any(|f| f.label() == prost_types::field_descriptor_proto::Label::Repeated)
    });
    if collection && result == Some(ResultShape::One) {
        refused.push(DescriptorRefusal::UnboundedCollection(p()));
    }
    if raw.description.trim().is_empty() {
        refused.push(DescriptorRefusal::DescriptionMissing(p()));
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
        (Some(callers), Some(permission), Some(result), Some(effect), Some(presence))
            if refused.is_empty() =>
        {
            Ok((
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
                    examples: raw.examples,
                },
                route_key,
            ))
        }
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
        Some(wire::To::Keyed(k)) => Route::Keyed {
            group: k.group,
            key: k.key,
        },
        Some(wire::To::Any(_)) => Route::Any,
        Some(wire::To::LocalFirst(_)) | None => Route::LocalFirst,
    };
    Parsed {
        callers,
        permission,
        effect,
        presence,
        session,
        result,
        route,
    }
}

/// The message a fully qualified type name (`.pkg.Outer.Inner`) names, if the set holds it.
fn find_message<'a>(set: &'a FileDescriptorSet, type_name: &str) -> Option<&'a DescriptorProto> {
    let name = type_name.strip_prefix('.')?;
    set.file.iter().find_map(|file: &FileDescriptorProto| {
        let rest = if file.package().is_empty() {
            Some(name)
        } else {
            name.strip_prefix(file.package())?.strip_prefix('.')
        }?;
        let mut parts = rest.split('.');
        let outer = parts.next()?;
        let mut message = file.message_type.iter().find(|m| m.name() == outer)?;
        for part in parts {
            message = message.nested_type.iter().find(|m| m.name() == part)?;
        }
        Some(message)
    })
}

/// The anemnez options of one raw `MethodDescriptorProto`: field 4 (`options`), and within it every
/// occurrence of field 51701, concatenated, which protobuf merges as one message.
fn method_options(raw_method: &[u8]) -> Option<Vec<u8>> {
    let options = repeated(raw_method, 4).ok()?;
    let mut found: Option<Vec<u8>> = None;
    for block in options {
        for bytes in repeated(block, METHOD_OPTIONS_FIELD).ok()? {
            found.get_or_insert_with(Vec::new).extend_from_slice(bytes);
        }
    }
    found
}

/// Every length-delimited occurrence of field `number` in a raw message.
fn repeated(message: &[u8], number: u32) -> Result<Vec<&[u8]>, &'static str> {
    let mut out = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let (key, after) = varint(rest)?;
        rest = after;
        let field = u32::try_from(key >> 3).map_err(|_| "field number too large")?;
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
        #[prost(string, repeated, tag = "12")]
        pub examples: Vec<String>,
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

    #[derive(Clone, Copy, PartialEq, prost::Message)]
    pub struct Empty {}
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

    fn method(
        name: &str,
        input: &str,
        output: &str,
        options: Option<&wire::MethodOptions>,
    ) -> Vec<u8> {
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
        let mut service = ServiceDescriptorProto {
            name: Some("Herald".into()),
            ..Default::default()
        }
        .encode_to_vec();
        for m in methods {
            service.extend(ld(2, &m));
        }
        let mut file = FileDescriptorProto {
            name: Some("herald.proto".into()),
            package: Some("herald.v2".into()),
            message_type: vec![
                message(
                    "LandRequest",
                    &[
                        ("train", 1, Type::String, Label::Optional),
                        ("repo", 3, Type::String, Label::Optional),
                    ],
                ),
                message("LandReply", &[]),
                message(
                    "QueueReply",
                    &[("trains", 1, Type::String, Label::Repeated)],
                ),
            ],
            ..Default::default()
        }
        .encode_to_vec();
        file.extend(ld(6, &service));
        ld(1, &file)
    }

    fn good() -> wire::MethodOptions {
        wire::MethodOptions {
            description: "Land a train on its repo's main.".into(),
            result: Some(wire::Result {
                shape: Some(wire::Shape::One(wire::Empty {})),
            }),
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
        land.examples = vec!["herald.land{repo = 'agora'}".into()];
        land.route = Some(wire::Route {
            to: Some(wire::To::Keyed(wire::Leader {
                group: "herald-repo".into(),
                key: "repo".into(),
            })),
        });
        let mut queue = good();
        queue.effect = 1;
        queue.result = Some(wire::Result {
            shape: Some(wire::Shape::Cursor(wire::Empty {})),
        });
        queue.route = Some(wire::Route {
            to: Some(wire::To::Any(wire::Empty {})),
        });
        let bytes = set(vec![
            method(
                "Land",
                ".herald.v2.LandRequest",
                ".herald.v2.LandReply",
                Some(&land),
            ),
            method(
                "Queue",
                ".herald.v2.LandRequest",
                ".herald.v2.QueueReply",
                Some(&queue),
            ),
            method(
                "Internal",
                ".herald.v2.LandRequest",
                ".herald.v2.LandReply",
                None,
            ),
        ]);
        let methods = check_descriptor(&bytes).expect("admitted");
        let paths: Vec<_> = methods.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/herald.v2.Herald/Land", "/herald.v2.Herald/Queue"],
            "Internal has no face"
        );
        assert_eq!(
            methods[0].route_key,
            Some(RouteKey {
                name: "repo".into(),
                number: 3
            })
        );
        assert_eq!(
            methods[0].options.route,
            Route::Keyed {
                group: "herald-repo".into(),
                key: "repo".into()
            }
        );
        assert!(methods[0].options.blocks && !methods[0].options.runs_to_completion);
        assert_eq!(methods[0].options.examples, ["herald.land{repo = 'agora'}"]);
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
            to: Some(wire::To::Keyed(wire::Leader {
                group: "herald-repo".into(),
                key: "nope".into(),
            })),
        });
        let mut lost = keyed.clone();
        lost.route = Some(wire::Route {
            to: Some(wire::To::Keyed(wire::Leader {
                group: "g".into(),
                key: "repo".into(),
            })),
        });
        let bytes = set(vec![
            method(
                "Bare",
                ".herald.v2.LandRequest",
                ".herald.v2.LandReply",
                Some(&bare),
            ),
            method(
                "Person",
                ".herald.v2.LandRequest",
                ".herald.v2.LandReply",
                Some(&person),
            ),
            method(
                "Keyed",
                ".herald.v2.LandRequest",
                ".herald.v2.LandReply",
                Some(&keyed),
            ),
            method(
                "Queue",
                ".herald.v2.LandRequest",
                ".herald.v2.QueueReply",
                Some(&good()),
            ),
            method(
                "Lost",
                ".herald.v2.Missing",
                ".herald.v2.LandReply",
                Some(&lost),
            ),
        ]);
        let refused = check_descriptor(&bytes).expect_err("refused");
        let named: Vec<(&str, String)> = refused
            .iter()
            .map(|r| {
                (
                    r.name(),
                    r.to_string()
                        .split(':')
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                )
            })
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
        outer.nested_type.push(message(
            "Inner",
            &[("repo", 7, Type::String, Label::Optional)],
        ));
        let set = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                package: Some("p.v1".into()),
                message_type: vec![message("First", &[]), outer],
                ..Default::default()
            }],
        };
        assert_eq!(
            find_message(&set, ".p.v1.First").map(DescriptorProto::name),
            Some("First")
        );
        assert_eq!(
            find_message(&set, ".p.v1.Outer.Inner").map(DescriptorProto::name),
            Some("Inner")
        );
        assert_eq!(
            find_message(&set, ".p.v1.Inner"),
            None,
            "a nested type is not top-level"
        );
        assert_eq!(find_message(&set, ".q.v1.First"), None, "another package");
    }
}
