//! `check_described`: the descriptor and its event registry, checked together, every refusal at once (CONTRACT.md §1a,
//! `a-registry-is-checked-as-the-descriptor-is`). `check_descriptor` alone keeps its answer for a reader that doesn't
//! read the registry; this is that check plus the registry's own refusals and the methods' `causes` against it.
//!
//! What an entry says is hydramq-message's (`registry::entry_schema`, `registry::check_entry`): read, never restated.

use std::borrow::Cow;

use hydramq_message::{Limits, Message, SCHEMA_TYPE, Schema, Symbol, SymbolTable, Value};

use crate::describe::{EventRegistry, Registry};
use crate::descriptor::{DescriptorRefusal, Method, check_descriptor};

/// A descriptor and its registry, admitted: the offered methods, and the kinds declared, in the registry's order
/// (empty when undeclared: [`Described::registry`] says which).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Described {
    pub methods: Vec<Method>,
    pub kinds: Vec<String>,
    /// Whether the product declared a registry: an undeclared one is said, never read as no events.
    pub declared: bool,
}

/// The descriptor `file_descriptor_set` and the registry served beside it, checked together.
///
/// # Errors
/// Every refusal, each named: the descriptor's own, then the registry's by entry, then each method's undeclared cause.
pub fn check_described(file_descriptor_set: &[u8], registry: &Registry) -> Result<Described, Vec<DescriptorRefusal>> {
    let (methods, mut refused) = match check_descriptor(file_descriptor_set) {
        Ok(methods) => (methods, Vec::new()),
        Err(refused) => (Vec::new(), refused),
    };
    let (declared, entries): (bool, &[Vec<u8>]) = match registry {
        Registry::Declared(entries) => (true, entries),
        _ => (false, &[]),
    };
    let mut kinds: Vec<String> = Vec::new();
    for (at, bytes) in entries.iter().enumerate() {
        match entry(at, bytes) {
            Ok(kind) if kinds.contains(&kind) => refused.push(DescriptorRefusal::KindTwice(kind)),
            Ok(kind) => kinds.push(kind),
            Err((kind, mut found)) => {
                refused.append(&mut found);
                kinds.extend(kind.filter(|k| !kinds.contains(k)));
            }
        }
    }
    for method in &methods {
        for cause in &method.options.causes {
            if !kinds.contains(cause) {
                refused.push(DescriptorRefusal::CauseUndeclared(method.path.clone(), cause.clone()));
            }
        }
    }
    if refused.is_empty() { Ok(Described { methods, kinds, declared }) } else { Err(refused) }
}

/// The registry a product serves, made from the entries it built: checked with its descriptor by [`check_described`],
/// then each entry's `causes` filled from the methods whose option 17 names its kind, by gRPC path in the
/// descriptor's order (G, rule 2: the methods are the one declaration, so the catalog and the digest can't disagree
/// with them). A kind no method causes gets an empty list: said, never absent. The entries keep their order, and each
/// is encoded canonically, so the same build always serves the same bytes (CONTRACT.md `a-registry-is-byte-stable`).
///
/// # Errors
/// Every refusal [`check_described`] finds.
pub fn registry_of(file_descriptor_set: &[u8], entries: Vec<Vec<u8>>) -> Result<EventRegistry, Vec<DescriptorRefusal>> {
    let described = check_described(file_descriptor_set, &Registry::Declared(entries.clone()))?;
    let table = SymbolTable::system();
    let mut filled = Vec::with_capacity(entries.len());
    for (bytes, kind) in entries.iter().zip(&described.kinds) {
        let mut message = hydramq_message::decode_message(bytes, &Limits::default(), &table)
            .map_err(|e| vec![DescriptorRefusal::EntryUnread(filled.len(), e.to_string())])?;
        let causes = causes_for(&described.methods, kind).into_iter().map(|p| Value::String(Cow::Owned(p))).collect();
        if let Some(Value::Struct(fields)) = &mut message.body {
            fields.push((Symbol::from("causes"), Value::List(causes)));
        }
        let encoded = hydramq_message::encode_message_canonical(&message)
            .map_err(|e| vec![DescriptorRefusal::EntryInvalid(kind.clone(), format!("re-encoding it: {e}"))])?;
        filled.push(encoded);
    }
    Ok(EventRegistry::new(filled))
}

/// A served registry as a reader of the reply takes it (D m/48108): each entry must carry `causes`, equal to what the
/// methods say ([`registry_of`] filled them), and with them taken out it is checked as a built entry is, by
/// [`check_described`]. So a reader never refuses what the serving face served as `causes-written`, and a served
/// entry whose causes disagree with the descriptor is refused by name, `causes-mismatch`. An undeclared registry is
/// checked as [`check_described`] checks it.
///
/// # Errors
/// Every refusal [`check_described`] finds in the built form, and each `causes-mismatch`.
pub fn check_served(file_descriptor_set: &[u8], registry: &Registry) -> Result<Described, Vec<DescriptorRefusal>> {
    let Registry::Declared(served) = registry else { return check_described(file_descriptor_set, registry) };
    let table = SymbolTable::system();
    let (mut built, mut said) = (Vec::with_capacity(served.len()), Vec::with_capacity(served.len()));
    for bytes in served {
        // One that doesn't read is handed on as it is, so check_described says it, once, by its place.
        let Ok(mut message) = hydramq_message::decode_message(bytes, &Limits::default(), &table) else {
            built.push(bytes.clone());
            said.push(None);
            continue;
        };
        let mut causes = None;
        if let Some(Value::Struct(fields)) = &mut message.body
            && let Some(at) = fields.iter().position(|(k, _)| matches!(k, Symbol::Text(t) if t == "causes"))
        {
            let (_, value) = fields.remove(at);
            causes = Some(match value {
                Value::List(paths) => paths
                    .iter()
                    .map(|p| if let Value::String(p) = p { p.to_string() } else { format!("{p:?}") })
                    .collect(),
                other => vec![format!("{other:?}")],
            });
        }
        built.push(hydramq_message::encode_message_canonical(&message).unwrap_or_else(|_| bytes.clone()));
        said.push(causes);
    }
    let described = check_described(file_descriptor_set, &Registry::Declared(built))?;
    let refused: Vec<_> = described
        .kinds
        .iter()
        .zip(said)
        .filter_map(|(kind, said)| {
            let derived = causes_for(&described.methods, kind);
            (said.as_ref() != Some(&derived)).then(|| DescriptorRefusal::CausesMismatch(kind.clone(), said, derived))
        })
        .collect();
    if refused.is_empty() { Ok(described) } else { Err(refused) }
}

/// The methods that may publish `kind`: each whose option 17 names it, by gRPC path, in the descriptor's order. The
/// one derivation the serving face fills and a reader checks against.
fn causes_for(methods: &[Method], kind: &str) -> Vec<String> {
    methods.iter().filter(|m| m.options.causes.iter().any(|c| c == kind)).map(|m| m.path.clone()).collect()
}

/// One entry, read and checked: its kind, or every refusal it earns.
fn entry(at: usize, bytes: &[u8]) -> Result<String, (Option<String>, Vec<DescriptorRefusal>)> {
    let table = SymbolTable::system();
    let message = hydramq_message::decode_message(bytes, &Limits::default(), &table)
        .map_err(|e| (None, vec![DescriptorRefusal::EntryUnread(at, e.to_string())]))?;
    if message.kind.as_deref() != Some(hydramq_message::ENTRY_TYPE) {
        let said = message.kind.as_deref().unwrap_or("(none)").to_owned();
        return Err((None, vec![DescriptorRefusal::EntryNotAnEntry(at, said)]));
    }
    let body = message.body.as_ref();
    let kind = match body.and_then(|b| field(b, "kind")) {
        Some(Value::String(k)) => k.to_string(),
        _ => format!("registry entry {at}"),
    };
    let report = hydramq_message::validate(&hydramq_message::entry_schema(), &message);
    if !report.complies() {
        let what = report.violations.iter().map(|v| format!("{}: {}", v.path, v.what)).collect::<Vec<_>>().join("; ");
        return Err((None, vec![DescriptorRefusal::EntryInvalid(kind, what)]));
    }
    let Some(body) = body else { return Err((None, vec![DescriptorRefusal::EntryInvalid(kind, "no body".into())])) };
    let mut refused = Vec::new();
    if let Err(words) = hydramq_message::check_entry(body) {
        refused.push(DescriptorRefusal::DeprecatedWithoutReplacement(words));
    }
    if field(body, "causes").is_some() {
        refused.push(DescriptorRefusal::CausesWritten(kind.clone()));
    }
    let schema = field(body, "schema").map(|s| {
        Schema::from_message(&Message {
            kind: Some(Cow::Borrowed(SCHEMA_TYPE)),
            body: Some(s.clone()),
            ..Message::default()
        })
    });
    match schema {
        Some(Ok(schema)) => {
            if let Some(Value::List(examples)) = field(body, "examples") {
                for example in examples {
                    refused.extend(example_refused(&kind, &schema, example));
                }
            }
        }
        Some(Err(e)) => refused.push(DescriptorRefusal::EntryInvalid(kind.clone(), format!("its schema: {e}"))),
        None => refused.push(DescriptorRefusal::EntryInvalid(kind.clone(), "no schema".into())),
    }
    // Refused here, its kind still reads: it stays declared, so a method's cause of it isn't also said undeclared.
    if refused.is_empty() { Ok(kind) } else { Err((Some(kind), refused)) }
}

/// An example whose body its kind's schema refuses, said with its caption.
fn example_refused(kind: &str, schema: &Schema, example: &Value<'_>) -> Option<DescriptorRefusal> {
    let caption = match field(example, "caption") {
        Some(Value::String(c)) => c.to_string(),
        _ => String::new(),
    };
    let message = Message {
        kind: Some(Cow::Owned(schema.name.clone())),
        body: field(example, "body").cloned(),
        ..Message::default()
    };
    let report = hydramq_message::validate(schema, &message);
    (!report.complies()).then(|| {
        let what = report.violations.iter().map(|v| format!("{}: {}", v.path, v.what)).collect::<Vec<_>>().join("; ");
        DescriptorRefusal::ExampleInvalid(kind.to_owned(), caption, what)
    })
}

/// A struct value's field `name`, if it has one.
fn field<'b, 'a>(value: &'b Value<'a>, name: &str) -> Option<&'b Value<'a>> {
    let Value::Struct(fields) = value else { return None };
    fields.iter().find(|(k, _)| matches!(k, Symbol::Text(t) if t == name)).map(|(_, v)| v)
}
