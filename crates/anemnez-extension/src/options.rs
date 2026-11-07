//! The options an extension writes on its methods and services (CONTRACT.md
//! `#a-product-describes-itself-from-its-idl`), as plain Rust values.

use serde::{Deserialize, Serialize};

/// Whether a method changes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Effect {
    Read,
    Write,
}

/// How prominently a caller's tool surface lists a method (Substrate's four tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Presence {
    Full,
    Minimal,
    Index,
    Hidden,
}

/// The extension's suggested permission, read by an agent with no grant of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Permission {
    Allow,
    Ask,
    Deny,
}

/// Who may call a method. Only `Agent` methods get an agent face (Lua, MCP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Callers {
    Agent,
    Product,
    Owner,
}

/// For state that outlives one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Session {
    /// Answers a session id.
    Opens,
    /// Takes one.
    Within,
    /// Takes one, and ends it.
    Closes,
}

/// Where a cursor means something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CursorScope {
    /// The events are replicated: a cursor resumes at any instance.
    Swarm,
    /// A cursor carries its issuing instance and is refused elsewhere (`cursor-not-mine`).
    Instance,
}

/// How a method's result comes back: a collection is a cursor or capped, never unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultShape {
    Cursor,
    Bound(u32),
    One,
}

/// Where a call goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// The instance leading this election group.
    Leader(String),
    /// This host's instance first.
    #[default]
    LocalFirst,
    Any,
    /// The instance holding this call's `key` in `group`: one leader per key. `key` names a
    /// top-level string field of the method's input ([`check_route_key`] at connect); the bridge
    /// resolves its value's holder on every call, and refuses an empty value with
    /// [`crate::refusal::NO_KEY`].
    Keyed {
        group: String,
        key: String,
    },
}

/// The field a keyed route reads its key from, as [`check_route_key`] found it: the name, and the
/// field number, so a bridge reading wire bytes finds the key without decoding the whole call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteKey {
    pub name: String,
    pub number: i32,
}

/// Why a keyed route's descriptor is refused at connect. Each names the method's input message and
/// the key, so the product's author knows what to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteKeyRefusal {
    /// The input message has no top-level field by that name. Nested paths are not supported yet.
    NoSuchField { message: String, key: String },
    /// The field is not a string.
    NotAString { message: String, key: String },
    /// The field is repeated: a call must carry exactly one key.
    Repeated { message: String, key: String },
}

impl std::fmt::Display for RouteKeyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteKeyRefusal::NoSuchField { message, key } => {
                write!(f, "route key '{key}' is not a top-level field of {message}")
            }
            RouteKeyRefusal::NotAString { message, key } => {
                write!(f, "route key '{key}' of {message} is not a string")
            }
            RouteKeyRefusal::Repeated { message, key } => {
                write!(f, "route key '{key}' of {message} is repeated")
            }
        }
    }
}

impl std::error::Error for RouteKeyRefusal {}

/// The connect-time check for a keyed route (CONTRACT.md `#a-keyed-route-names-a-string-field`): `key`
/// must be a top-level, non-repeated string field of the method's `input` message. proto3 has no
/// `required`, so a present-but-empty key is the bridge's refusal per call, not this one's. One pure
/// function, so every bridge refuses a descriptor alike.
pub fn check_route_key(
    input: &prost_types::DescriptorProto,
    key: &str,
) -> Result<RouteKey, RouteKeyRefusal> {
    use prost_types::field_descriptor_proto::{Label, Type};
    let message = input.name().to_owned();
    let field = input
        .field
        .iter()
        .find(|f| f.name() == key)
        .ok_or_else(|| RouteKeyRefusal::NoSuchField {
            message: message.clone(),
            key: key.to_owned(),
        })?;
    if field.r#type() != Type::String {
        return Err(RouteKeyRefusal::NotAString {
            message,
            key: key.to_owned(),
        });
    }
    if field.label() == Label::Repeated {
        return Err(RouteKeyRefusal::Repeated {
            message,
            key: key.to_owned(),
        });
    }
    Ok(RouteKey {
        name: key.to_owned(),
        number: field.number(),
    })
}

/// Everything written on one method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodOptions {
    pub description: String,
    pub result: ResultShape,
    pub effect: Effect,
    #[serde(default)]
    pub route: Route,
    #[serde(default)]
    pub person_act: bool,
    pub presence: Presence,
    /// Required: every method states one.
    pub permission: Permission,
    /// Required: nothing gets an agent face by omission.
    pub callers: Callers,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Session>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::field_descriptor_proto::{Label, Type};
    use prost_types::{DescriptorProto, FieldDescriptorProto};

    fn input(fields: &[(&str, i32, Type, Label)]) -> DescriptorProto {
        DescriptorProto {
            name: Some("LandRequest".into()),
            field: fields
                .iter()
                .map(|(name, number, ty, label)| FieldDescriptorProto {
                    name: Some((*name).into()),
                    number: Some(*number),
                    r#type: Some(*ty as i32),
                    label: Some(*label as i32),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// a-keyed-route-names-a-string-field: the key is a top-level, non-repeated string field, and
    /// the check answers its number with its name.
    #[test]
    fn a_keyed_route_names_a_top_level_string_field() {
        let land = input(&[
            ("train", 1, Type::String, Label::Optional),
            ("repo", 3, Type::String, Label::Optional),
            ("depth", 4, Type::Uint32, Label::Optional),
            ("tags", 5, Type::String, Label::Repeated),
        ]);
        assert_eq!(
            check_route_key(&land, "repo"),
            Ok(RouteKey {
                name: "repo".into(),
                number: 3
            })
        );
        let refused = |key: &str| check_route_key(&land, key).expect_err("refused");
        assert!(matches!(
            refused("missing"),
            RouteKeyRefusal::NoSuchField { .. }
        ));
        assert!(
            matches!(refused("repo.name"), RouteKeyRefusal::NoSuchField { .. }),
            "no nested paths yet"
        );
        assert!(matches!(
            refused("depth"),
            RouteKeyRefusal::NotAString { .. }
        ));
        assert!(matches!(refused("tags"), RouteKeyRefusal::Repeated { .. }));
        assert_eq!(
            refused("depth").to_string(),
            "route key 'depth' of LandRequest is not a string"
        );
    }

    #[test]
    fn a_keyed_route_reads_and_writes_by_name() {
        let route = Route::Keyed {
            group: "herald-repo".into(),
            key: "repo".into(),
        };
        let text = serde_json::to_string(&route).expect("writes");
        assert_eq!(text, r#"{"keyed":{"group":"herald-repo","key":"repo"}}"#);
        assert_eq!(serde_json::from_str::<Route>(&text).expect("reads"), route);
    }

    #[test]
    fn permission_and_callers_are_required_when_read() {
        let without = r#"{"description":"d","result":"one","effect":"READ","presence":"FULL"}"#;
        assert!(
            serde_json::from_str::<MethodOptions>(without).is_err(),
            "no default for either"
        );
        let with = r#"{"description":"d","result":"one","effect":"READ","presence":"FULL","permission":"ASK","callers":"AGENT"}"#;
        let read: MethodOptions = serde_json::from_str(with).expect("reads");
        assert_eq!(
            (read.route, read.person_act, read.session),
            (Route::LocalFirst, false, None)
        );
    }
}
