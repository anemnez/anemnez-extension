//! The `events` tool's `catalog` (CONTRACT.md §7 `the-events-tool-lists-the-registry`; §1a; ROUTING §3.4, AX10): what
//! a product tells, listed from the registry its `Describe` served, on every face by the same arguments. Pure: a host
//! parses the tool's arguments with [`events_call`] and answers a catalog with [`catalog`] from the registry it read.
//! Kinds are reached by calling it, never listed in `tools/list` or the instructions (H m/38028).

use serde_json::{Value as Json, json};

use hydramq_message::{Limits, Symbol, SymbolTable, Value};

use crate::describe::Registry;

/// The `events` tool's arguments its host answers itself, from the registry, never encoding them into the product's
/// `EventsRequest`: a call naming `catalog` is parsed by [`events_call`] before anything is sent (O m/41925's rule
/// holds for the rest: every other argument is a field of the contract's request).
pub const CATALOG_ARGUMENTS: [&str; 2] = ["catalog", "kind"];

/// The refusal of an `events` call that asks for both a catalog and a read.
pub const CATALOG_OR_READ: &str = "catalog-or-read";

/// The refusal of a catalog asked for a kind the registry doesn't declare.
pub const KIND_UNKNOWN: &str = "kind-unknown";

/// What an `events` tool call asks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventsCall {
    /// A page of events after `after`, of `kinds` when named.
    Read { after: String, kinds: Vec<String> },
    /// The registry's kinds, or one `kind` whole (its schema and examples too).
    Catalog { kind: Option<String> },
}

/// The `events` tool's arguments as the call they ask: `catalog: true` (with an optional `kind`) is a catalog, anything
/// else a read. `catalog` beside `after` or `kinds` is refused, never guessed.
///
/// # Errors
/// [`CATALOG_OR_READ`], or an argument of the wrong type, by name.
pub fn events_call(arguments: &Json) -> Result<EventsCall, String> {
    let object = arguments.as_object().ok_or("the events tool's arguments are a JSON object")?;
    let text = |name: &str| match object.get(name) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{name}` is a string")),
    };
    let catalog = match object.get("catalog") {
        None | Some(Json::Null | Json::Bool(false)) => false,
        Some(Json::Bool(true)) => true,
        Some(_) => return Err("`catalog` is a boolean".to_owned()),
    };
    if catalog {
        if object.contains_key("after") || object.contains_key("kinds") {
            return Err(format!("{CATALOG_OR_READ}: ask for the catalog or a page of events, not both in one call"));
        }
        return Ok(EventsCall::Catalog { kind: text("kind")? });
    }
    let kinds = match object.get("kinds") {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Array(kinds)) => kinds
            .iter()
            .map(|k| k.as_str().map(str::to_owned).ok_or_else(|| "`kinds` is a list of strings".to_owned()))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`kinds` is a list of strings".to_owned()),
    };
    Ok(EventsCall::Read { after: text("after")?.unwrap_or_default(), kinds })
}

/// The catalog of `registry` (as served, its causes filled), or of its one `kind`. An undeclared registry says
/// `events undeclared`, an empty one `no event kinds declared` (AX5): never an empty list that reads as no events.
/// Each kind says its `kind`, `description`, `version`, `stability`, `asks` (`asks unknown` when absent, never none:
/// AX8), `volume` (`volume undeclared` when absent), `causes` and `focus`; one asked by name adds its schema and
/// examples.
///
/// # Errors
/// [`KIND_UNKNOWN`], naming the declared kinds; or an entry that doesn't read, by its place.
pub fn catalog(registry: &Registry, kind: Option<&str>) -> Result<Json, String> {
    let entries = match registry {
        Registry::Declared(entries) => entries,
        _ => return Ok(json!({ "said": Registry::Undeclared.to_string(), "kinds": [] })),
    };
    let table = SymbolTable::system();
    let mut kinds = Vec::with_capacity(entries.len());
    for (at, bytes) in entries.iter().enumerate() {
        let message = hydramq_message::decode_message(bytes, &Limits::default(), &table)
            .map_err(|e| format!("registry entry {at} doesn't read: {e}"))?;
        let body = message.body.unwrap_or(Value::Struct(Vec::new()));
        kinds.push(listed(&body));
    }
    let Some(asked) = kind else { return Ok(json!({ "said": registry.to_string(), "kinds": kinds })) };
    let at = kinds.iter().position(|k| k["kind"] == asked).ok_or_else(|| {
        let declared: Vec<_> = kinds.iter().filter_map(|k| k["kind"].as_str()).collect();
        format!("{KIND_UNKNOWN}: {asked:?} isn't declared; the kinds are {}", declared.join(", "))
    })?;
    let table = SymbolTable::system();
    let body = hydramq_message::decode_message(&entries[at], &Limits::default(), &table)
        .map_err(|e| format!("registry entry {at} doesn't read: {e}"))?
        .body
        .unwrap_or(Value::Struct(Vec::new()));
    let mut whole = kinds.swap_remove(at);
    for name in ["schema", "examples"] {
        if let Some(value) = field(&body, name) {
            whole[name] = serde_json::from_str(&hydramq_message::to_json(value)).unwrap_or(Json::Null);
        }
    }
    Ok(json!({ "said": registry.to_string(), "kinds": [whole] }))
}

/// One kind's listing.
fn listed(body: &Value<'_>) -> Json {
    let text = |name: &str| match field(body, name) {
        Some(Value::String(s)) => Some(s.to_string()),
        Some(Value::Symbol(Symbol::Text(s))) => Some(s.to_string()),
        _ => None,
    };
    let number = |name: &str| field(body, name).map(|v| serde_json::from_str::<Json>(&hydramq_message::to_json(v)));
    let causes: Vec<String> = match field(body, "causes") {
        Some(Value::List(paths)) => {
            paths.iter().filter_map(|p| if let Value::String(p) = p { Some(p.to_string()) } else { None }).collect()
        }
        _ => Vec::new(),
    };
    json!({
        "kind": text("kind"),
        "description": text("description"),
        "version": number("version").and_then(Result::ok),
        "stability": text("stability"),
        "asks": text("asks").unwrap_or_else(|| "asks unknown".to_owned()),
        "volume": number("volume").and_then(Result::ok).unwrap_or_else(|| json!("volume undeclared")),
        "causes": causes,
        "focus": matches!(field(body, "focus"), Some(Value::Bool(true))),
    })
}

/// A struct value's field `name`, if it has one.
fn field<'b, 'a>(value: &'b Value<'a>, name: &str) -> Option<&'b Value<'a>> {
    let Value::Struct(fields) = value else { return None };
    fields.iter().find(|(k, _)| matches!(k, Symbol::Text(t) if t == name)).map(|(_, v)| v)
}
