//! Health: state, with parts, and the events cursor it is true as of
//! (CONTRACT.md `#health-is-state-with-parts`).

use serde::{Deserialize, Serialize};

/// A component's state, or one part's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    /// Not yet checked, or could not be asked.
    #[default]
    Unknown,
    /// Coming up: not an error, and never counted as failing.
    Starting,
    /// Fully working.
    Serving,
    /// Working with less: a part is down, a dependency is slow.
    Degraded,
    /// Cannot do its work.
    Failing,
    /// Finishing work in hand before a stop or an upgrade.
    Draining,
    /// Nothing in hand.
    Drained,
}

impl State {
    /// Every state, in the proto's order.
    pub const ALL: [State; 7] = [
        State::Unknown,
        State::Starting,
        State::Serving,
        State::Degraded,
        State::Failing,
        State::Draining,
        State::Drained,
    ];

    /// Serving, to a host deciding whether an upgrade took: `SERVING` and `DEGRADED` both are. The
    /// component is the judge of whether it can do its work; one that should be rolled back says
    /// `FAILING`.
    pub fn is_serving(self) -> bool {
        matches!(self, State::Serving | State::Degraded)
    }

    /// The name the proto gives it.
    pub fn proto_name(self) -> &'static str {
        match self {
            State::Unknown => "UNKNOWN",
            State::Starting => "STARTING",
            State::Serving => "SERVING",
            State::Degraded => "DEGRADED",
            State::Failing => "FAILING",
            State::Draining => "DRAINING",
            State::Drained => "DRAINED",
        }
    }
}

/// One part of a component's health, named by path (`memory/index`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub state: State,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Unix milliseconds.
    pub since: i64,
}

/// What restarting a component would cost now, in its own words (`HealthReply.restart_cost`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartCost {
    /// One sentence a person reads.
    pub line: String,
    /// The component's own measured estimate of seconds back to serving; `None` when unmeasured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secs: Option<u32>,
}

/// `Health/Get`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Health {
    pub state: State,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    /// The events cursor this answer is true as of: subscribe after it for every change since.
    pub as_of: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// What a restart would cost now; `None`: name the product only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_cost: Option<RestartCost>,
}

impl Health {
    /// The part at `path`, if the component reported it.
    pub fn part(&self, path: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serving_and_degraded_both_serve_and_nothing_else_does() {
        let serving: Vec<State> = State::ALL.into_iter().filter(|s| s.is_serving()).collect();
        assert_eq!(serving, [State::Serving, State::Degraded]);
    }

    #[test]
    fn a_health_answer_reads_and_writes_by_the_protos_names() {
        let health = Health {
            state: State::Degraded,
            parts: vec![Part {
                name: "memory/index".into(),
                state: State::Failing,
                detail: "rebuilding".into(),
                since: 1,
            }],
            as_of: "c-42".into(),
            detail: String::new(),
            restart_cost: Some(RestartCost {
                line: "qwen-35b warm".into(),
                secs: None,
            }),
        };
        let text = serde_json::to_string(&health).expect("writes");
        assert!(
            text.contains(r#""state":"DEGRADED""#) && text.contains(r#""state":"FAILING""#),
            "{text}"
        );
        let back: Health = serde_json::from_str(&text).expect("reads");
        assert_eq!(back, health);
        assert_eq!(
            back.part("memory/index").map(|p| p.state),
            Some(State::Failing)
        );
        assert!(
            text.contains(r#""restart_cost":{"line":"qwen-35b warm"}"#),
            "{text}"
        );
        let old: Health = serde_json::from_str(r#"{"state":"SERVING","as_of":"c"}"#)
            .expect("an older answer reads");
        assert_eq!(old.restart_cost, None, "absent: the product's name only");
    }
}
