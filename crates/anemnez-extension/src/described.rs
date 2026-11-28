//! `check_described`: the descriptor and its event registry, checked together, every refusal at once (CONTRACT.md §1a,
//! `a-registry-is-checked-as-the-descriptor-is`). `check_descriptor` alone keeps its answer for a reader that doesn't
//! read the registry; this is that check plus the registry's own refusals and the methods' `causes` against it.
//!
//! What an entry says is hydramq-message's (`registry::entry_schema`, `registry::check_entry`): read, never restated.

use std::borrow::Cow;

use hydramq_message::{Limits, Message, SCHEMA_TYPE, Schema, Symbol, SymbolTable, Value};

use crate::describe::Registry;
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
            Err(mut found) => refused.append(&mut found),
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

/// One entry, read and checked: its kind, or every refusal it earns.
fn entry(at: usize, bytes: &[u8]) -> Result<String, Vec<DescriptorRefusal>> {
    let table = SymbolTable::system();
    let message = hydramq_message::decode_message(bytes, &Limits::default(), &table)
        .map_err(|e| vec![DescriptorRefusal::EntryUnread(at, e.to_string())])?;
    if message.kind.as_deref() != Some(hydramq_message::ENTRY_TYPE) {
        let said = message.kind.as_deref().unwrap_or("(none)").to_owned();
        return Err(vec![DescriptorRefusal::EntryNotAnEntry(at, said)]);
    }
    let body = message.body.as_ref();
    let kind = match body.and_then(|b| field(b, "kind")) {
        Some(Value::String(k)) => k.to_string(),
        _ => format!("registry entry {at}"),
    };
    let report = hydramq_message::validate(&hydramq_message::entry_schema(), &message);
    if !report.complies() {
        let what = report.violations.iter().map(|v| format!("{}: {}", v.path, v.what)).collect::<Vec<_>>().join("; ");
        return Err(vec![DescriptorRefusal::EntryInvalid(kind, what)]);
    }
    let Some(body) = body else { return Err(vec![DescriptorRefusal::EntryInvalid(kind, "no body".into())]) };
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
    if refused.is_empty() { Ok(kind) } else { Err(refused) }
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
