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
