//! The flattened face: what a descriptor's methods are as MCP tools, once (CONTRACT.md §7; B, C, D,
//! E, H, 2026-11-13). Agora calls it for every product it hosts, and a product serving its own MCP
//! face calls it too, so a hand-written face is checked against this function's output. Pure: no
//! clock, no store, no runtime.

use serde_json::{Value, json};

use crate::descriptor::{Field, FieldKind, Method};
use crate::exposure::{Face, exposure};
use crate::options::{Effect, Permission, ResultShape, ServiceOptions, Session};

/// The tool every flattened face has: the product's `Events`, read as one bounded batch.
pub const EVENTS_TOOL: &str = "events";

/// The path the events tool reads (`events.proto`).
pub const EVENTS_PATH: &str = "/anemnez.extension.v1.Events/Events";

/// What a tool is to its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolKind {
    /// One unary call of the method.
    Call,
    /// The product's `Events` stream, read for at most `max` events and a bounded wait.
    Events,
}

/// One MCP tool, unqualified: the host names it `<namespace>__<name>`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Tool {
    pub name: String,
    pub kind: ToolKind,
    /// The method's path, which a call's arguments are encoded for.
    pub path: String,
    pub description: String,
    /// A JSON Schema object for the tool's `arguments`.
    pub input_schema: Value,
    pub effect: Effect,
    /// The method's default consent (`exposure`).
    pub consent: Permission,
    /// `blocks` or `runs_to_completion`: work that can outlast a turn, answered with a job and its outcome
    /// as an event, never a wait (CONTRACT.md `#long-work-flattens-to-a-job-and-an-event`).
    pub long: bool,
    /// The method's result: a `cursor` result is a page tool, whose request carries the cursor and
    /// whose reply the next one.
    pub result: ResultShape,
    /// The method's part in a session: `opens` answers an id that `within` and `closes` take as a
    /// field of their request.
    pub session: Option<Session>,
}

impl Tool {
    /// The tool as `tools/list` lists it, unqualified: MCP's hints from the effect, and what the
    /// schema cannot say under `_meta.anemnez`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let read_only = self.effect == Effect::Read;
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": read_only,
                "destructiveHint": !read_only,
                "openWorldHint": self.effect == Effect::Act,
            },
            "_meta": { "anemnez": {
                "path": self.path,
                "effect": self.effect,
                "consent": self.consent,
                "long": self.long,
                "result": self.result,
                "session": self.session,
            } },
        })
    }
}

/// Why a descriptor has no flattened face. Each names what it refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FlattenRefusal {
    /// Two methods flatten to one name: the name, and both paths.
    NameCollision(String, String, String),
    /// A method flattens to a reserved name: the path and the name.
    ReservedName(String, String),
}

impl FlattenRefusal {
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::NameCollision(..) => "name-collision",
            Self::ReservedName(..) => "reserved-name",
        }
    }
}

/// Why a call has no tool.
pub const NO_SUCH_TOOL: &str = "no-such-tool";

/// A method's op name: its last path segment in snake case (`GetURL` → `get_url`, `URLParser` →
/// `url_parser`), the name a Lua mount gives the op too.
#[must_use]
pub fn op_name(path: &str) -> String {
    let name: Vec<char> = path.rsplit('/').next().unwrap_or_default().chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in name.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev = name[i - 1];
            let next_lower = name.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Every agent method as a tool, then the events tool when the service keeps events
/// (`events_kept` > 0); or every refusal, in the methods' order.
pub fn flatten(service: &ServiceOptions, methods: &[Method]) -> Result<Vec<Tool>, Vec<FlattenRefusal>> {
    let mut tools: Vec<Tool> = Vec::new();
    let mut refusals = Vec::new();
    for method in methods {
        let exposed = exposure(&method.options);
        if exposed.face != Face::Agent {
            continue;
        }
        let name = op_name(&method.path);
        if name == EVENTS_TOOL {
            refusals.push(FlattenRefusal::ReservedName(method.path.clone(), name));
            continue;
        }
        if let Some(first) = tools.iter().find(|t| t.name == name) {
            refusals.push(FlattenRefusal::NameCollision(name, first.path.clone(), method.path.clone()));
            continue;
        }
        tools.push(Tool {
            name,
            kind: ToolKind::Call,
            path: method.path.clone(),
            description: description(method),
            input_schema: object(method.input.iter().map(|f| (f.name.clone(), field_schema(f)))),
            effect: method.options.effect,
            consent: exposed.consent,
            long: method.options.blocks || method.options.runs_to_completion,
            result: method.options.result,
            session: method.options.session,
        });
    }
    if !refusals.is_empty() {
        return Err(refusals);
    }
    if service.events_kept > 0 {
        tools.push(events_tool());
    }
    Ok(tools)
}

/// The tool a call names, by its unqualified name only: nothing the client sends picks a path.
#[must_use]
pub fn resolve<'a>(tools: &'a [Tool], name: &str) -> Option<&'a Tool> {
    tools.iter().find(|t| t.name == name)
}

fn description(method: &Method) -> String {
    let mut text = method.options.description.clone();
    if !method.options.examples.is_empty() {
        text.push_str("\n\nExamples:");
        for example in &method.options.examples {
            text.push_str(&format!("\n- {}: {}", example.caption, example.args_json));
        }
    }
    text
}

fn events_tool() -> Tool {
    Tool {
        name: EVENTS_TOOL.into(),
        kind: ToolKind::Events,
        path: EVENTS_PATH.into(),
        description: "The product's events after a cursor, up to the head and at most `max`: pass the answer's \
                      `next` as `after` to read on. Empty `after` starts at the oldest kept."
            .into(),
        input_schema: object([
            ("after".to_owned(), json!({ "type": "string" })),
            ("kinds".to_owned(), json!({ "type": "array", "items": { "type": "string" } })),
            ("max".to_owned(), json!({ "type": "integer", "minimum": 1 })),
        ]),
        effect: Effect::Read,
        consent: Permission::Allow,
        long: false,
        result: ResultShape::Cursor,
        session: None,
    }
}

/// A closed object schema: no key but these, since a typo must not become a different act.
fn object(properties: impl IntoIterator<Item = (String, Value)>) -> Value {
    let properties: serde_json::Map<String, Value> = properties.into_iter().collect();
    json!({ "type": "object", "properties": properties, "additionalProperties": false })
}

/// A field in the proto3 JSON mapping: a map is an object, a repeated field an array.
fn field_schema(field: &Field) -> Value {
    if field.map {
        return json!({ "type": "object" });
    }
    let one = kind_schema(&field.kind);
    if field.repeated { json!({ "type": "array", "items": one }) } else { one }
}

fn kind_schema(kind: &FieldKind) -> Value {
    match kind {
        FieldKind::Double | FieldKind::Float => json!({ "type": "number" }),
        FieldKind::Int32 | FieldKind::Uint32 | FieldKind::Sint32 | FieldKind::Fixed32 | FieldKind::Sfixed32 => {
            json!({ "type": "integer" })
        }
        FieldKind::Int64 | FieldKind::Uint64 | FieldKind::Sint64 | FieldKind::Fixed64 | FieldKind::Sfixed64 => {
            json!({ "type": ["integer", "string"] })
        }
        FieldKind::Bool => json!({ "type": "boolean" }),
        FieldKind::String => json!({ "type": "string" }),
        FieldKind::Bytes => json!({ "type": "string", "contentEncoding": "base64" }),
        FieldKind::Enum(name) => json!({ "type": ["string", "integer"], "description": format!("enum {name}") }),
        FieldKind::Message(name) => {
            well_known(name).unwrap_or_else(|| json!({ "type": "object", "description": format!("message {name}") }))
        }
    }
}

/// The well-known types the proto3 JSON mapping writes as something other than an object.
fn well_known(name: &str) -> Option<Value> {
    let scalar = |kind: &FieldKind| Some(kind_schema(kind));
    match name.strip_prefix(".google.protobuf.")? {
        "Timestamp" => Some(json!({ "type": "string", "format": "date-time" })),
        "Duration" | "FieldMask" => Some(json!({ "type": "string" })),
        "Struct" => Some(json!({ "type": "object" })),
        "ListValue" => Some(json!({ "type": "array" })),
        "Value" => Some(json!({})),
        "DoubleValue" | "FloatValue" => scalar(&FieldKind::Double),
        "Int32Value" | "UInt32Value" => scalar(&FieldKind::Int32),
        "Int64Value" | "UInt64Value" => scalar(&FieldKind::Int64),
        "BoolValue" => scalar(&FieldKind::Bool),
        "StringValue" => scalar(&FieldKind::String),
        "BytesValue" => scalar(&FieldKind::Bytes),
        _ => None,
    }
}

/// The binary gRPC metadata key a generated face's call carries the caller's stamp on: the JSON
/// `_meta.anemnez.caller` carries over MCP, as bytes.
pub const STAMP_METADATA_KEY: &str = "anemnez-caller-bin";

/// Who called through Agora: always an external agent, never the User. Every part is a claim that
/// attributes a call and authorizes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stamp {
    /// The lane the client says it works on.
    pub lane: Option<String>,
    /// The caller's directory, when the kernel proved it.
    pub cwd: Option<String>,
    /// The lane's lease on this host, held by the caller's own tree.
    pub lease: Option<StampLease>,
}

/// The lane's lease as a stamp carries it: its fencing token, and whether it is lapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StampLease {
    pub token: u64,
    pub lapsed: bool,
}

impl StampLease {
    #[must_use]
    pub fn new(token: u64, lapsed: bool) -> StampLease {
        StampLease { token, lapsed }
    }
}

/// A value read from a peer that nothing has vouched for. It has no accessor but
/// [`Untrusted::vouched_by_peer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untrusted<T>(T);

impl Untrusted<Stamp> {
    /// The stamp, trusted. Call this ONLY once the peer that sent it is known to be Agora's daemon:
    /// its one caller is the closed constructor that reads the peer at accept (CONTRACT.md
    /// `#an-external-caller-is-parsed-here-and-trusted-only-from-its-peer`). From any other peer a
    /// call is [`Stamp::unvouched`].
    #[must_use]
    pub fn vouched_by_peer(self) -> Stamp {
        self.0
    }
}

impl Stamp {
    /// External, with no lane, place or lease: what a call from any peer but Agora's daemon reads as.
    #[must_use]
    pub fn unvouched() -> Stamp {
        Stamp { lane: None, cwd: None, lease: None }
    }

    /// The stamp Agora writes. A lease with no lane is dropped.
    #[must_use]
    pub fn new(lane: Option<String>, cwd: Option<String>, lease: Option<StampLease>) -> Stamp {
        let lease = lease.filter(|_| lane.is_some());
        Stamp { lane, cwd, lease }
    }

    /// The stamp as `_meta.anemnez.caller` carries it.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let place = match &self.cwd {
            Some(cwd) => json!({ "cwd": cwd, "proven": true }),
            None => json!({ "proven": false }),
        };
        let mut stamp = json!({ "class": "external", "place": place });
        if let Some(lane) = &self.lane {
            stamp["lane"] = json!(lane);
            if let Some(lease) = self.lease {
                stamp[if lease.lapsed { "lease_lapsed" } else { "leased" }] = json!(true);
                stamp["lease"] = json!(lease.token);
            }
        }
        stamp
    }

    /// The stamp as [`STAMP_METADATA_KEY`] carries it.
    #[must_use]
    pub fn to_metadata(&self) -> Vec<u8> {
        self.to_json().to_string().into_bytes()
    }

    /// A peer's `_meta.anemnez.caller`, parsed and not trusted. Whatever does not read as Agora writes
    /// it reads as no lane, failing closed: not `external`, a lane that is not a `plain_label`, a
    /// lease with no lane or a token of 0.
    #[must_use]
    pub fn from_json(value: &Value) -> Untrusted<Stamp> {
        if value.get("class").and_then(Value::as_str) != Some("external") {
            return Untrusted(Stamp::unvouched());
        }
        let lane = value
            .get("lane")
            .and_then(Value::as_str)
            .filter(|l| crate::label::plain_label(l).is_ok())
            .map(str::to_owned);
        let place = value.get("place");
        let cwd = place
            .filter(|p| p.get("proven").and_then(Value::as_bool) == Some(true))
            .and_then(|p| p.get("cwd").and_then(Value::as_str))
            .map(str::to_owned);
        let flag = |key: &str| value.get(key).and_then(Value::as_bool) == Some(true);
        let lease = match (flag("leased"), flag("lease_lapsed"), value.get("lease").and_then(Value::as_u64)) {
            (true, false, Some(token)) if token > 0 => Some(StampLease::new(token, false)),
            (false, true, Some(token)) if token > 0 => Some(StampLease::new(token, true)),
            _ => None,
        };
        Untrusted(Stamp::new(lane, cwd, lease))
    }

    /// A call's [`STAMP_METADATA_KEY`] bytes, parsed and not trusted; bytes that are not a stamp read
    /// as no lane.
    #[must_use]
    pub fn from_metadata(bytes: &[u8]) -> Untrusted<Stamp> {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => Stamp::from_json(&value),
            Err(_) => Untrusted(Stamp::unvouched()),
        }
    }
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
