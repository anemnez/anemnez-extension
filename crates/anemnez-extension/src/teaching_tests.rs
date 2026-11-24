//! The outcome types' rows: Substrate's serde shapes byte for byte, the wire round trip, and the house-form message.

use super::*;

#[derive(Clone, Copy)]
enum Kinds {
    Detached,
}

impl InformKind for Kinds {
    fn as_str(self) -> &'static str {
        match self {
            Self::Detached => "process.detached",
        }
    }
}

/// C m/29800: RetryClass moves whole, its serde names byte-identical to Substrate's gate kernel, so a stored gate
/// decision reads the same through the shared type.
#[test]
fn retry_class_writes_substrates_own_names() {
    assert_eq!(serde_json::to_string(&RetryClass::Blanket).unwrap(), r#""Blanket""#);
    assert_eq!(serde_json::to_string(&RetryClass::Scoped).unwrap(), r#""Scoped""#);
    let stored: RetryClass = serde_json::from_str(r#""Scoped""#).unwrap();
    assert_eq!(stored, RetryClass::Scoped);
}

/// C m/29800: Inform's shape moves with the execute envelope's serde shape, byte for byte: an inform with no
/// obligation omits it, and an obligation always writes its holder.
#[test]
fn an_inform_round_trips_substrates_recorded_json_byte_for_byte() {
    let plain = r#"{"kind":"process.detached","text":"run 7 is detached: substrate.process.tail(7) to follow it","refs":["process:7"]}"#;
    let owed = r#"{"kind":"process.detached","text":"assess gap g","refs":["gap:g"],"obligation":{"id":"assess:g","until":"a verdict on gap:g","holder":"agent"}}"#;
    for recorded in [plain, owed] {
        let read: Inform = serde_json::from_str(recorded).expect("reads");
        assert_eq!(serde_json::to_string(&read).unwrap(), recorded, "written back byte for byte");
    }
    let made = Inform::new(Kinds::Detached, "assess gap g", vec!["gap:g".into()])
        .owed_by_agent("assess:g", "a verdict on gap:g");
    assert_eq!(serde_json::to_string(&made).unwrap(), owed);
    // An older record with no holder reads as the session's, as Substrate's own default.
    let older: Inform =
        serde_json::from_str(r#"{"kind":"k.x","text":"t","refs":[],"obligation":{"id":"i","until":"u"}}"#).unwrap();
    assert_eq!(older.obligation().map(Obligation::holder), Some(Holder::Session));
}

/// AX3: each outcome carries its own next move (a refusal its retry class, an error its recovery, Unclear only
/// resolve), whose it is to fix, and a house-form message an older reader matches by name.
#[test]
fn each_outcome_carries_its_own_next_move_and_whose_it_is() {
    let refused = Teaching::refused(
        "orient-takes-input",
        "an orient takes no argument: drop the request's fields",
        RetryClass::Blanket,
        Whose::Caller,
    )
    .with_shape("rpc Orient(OrientRequest) with an empty OrientRequest", "message OrientRequest {}");
    assert_eq!(refused.message(), "orient-takes-input: an orient takes no argument: drop the request's fields");
    assert_eq!(refused.qualified("ergon"), "ergon.orient-takes-input");
    assert_eq!(Teaching { code: "ergon.x".into(), ..refused.clone() }.qualified("agora"), "ergon.x", "kept as is");

    let later = Teaching::failed(
        "no-leader",
        "ergon has no leader right now, reloading",
        Recovery::Later { after_ms: 5_000 },
        Whose::Operator,
    );
    let defect = Teaching::failed(
        "stamp-not-from-agora",
        "a caller's stamp is believed only from Agora's judged peer",
        Recovery::Retry(RetryClass::Blanket),
        Whose::Defect { owner: "K".into(), key: "ergon:K/serve-0923".into() },
    );
    let unclear = Teaching::unclear("put-unanswered", "read the key back and judge by value", Whose::Caller);
    let json = |t: &Teaching| serde_json::to_value(t).unwrap();
    assert_eq!(json(&refused)["outcome"], serde_json::json!({"refused": "Blanket"}));
    assert_eq!(json(&later)["outcome"], serde_json::json!({"failed": {"later": {"after_ms": 5000}}}));
    assert_eq!(json(&defect)["whose"], serde_json::json!({"defect": {"owner": "K", "key": "ergon:K/serve-0923"}}));
    assert_eq!(json(&unclear)["outcome"], "unclear");
    assert_eq!(json(&unclear).get("shape"), None, "an empty shape is never written");

    for t in [refused, later, defect, unclear] {
        assert_eq!(Teaching::decode(&t.encode()), Ok(t.clone()), "the wire round trip of {}", t.code);
    }
}

/// Readers first (rule 8): a teaching whose outcome or whose this build doesn't know is refused by name, never read
/// as a default an agent would act on.
#[test]
fn a_teaching_this_build_cant_read_is_refused_by_name_never_defaulted() {
    let no_outcome = wire::Teaching {
        code: "x".into(),
        whose: Some(wire::Whose { who: Some(wire::Who::Caller(wire::Empty {})) }),
        ..Default::default()
    };
    assert!(Teaching::decode(&no_outcome.encode_to_vec()).is_err_and(|e| e.starts_with("teaching-outcome-unknown")));
    let no_whose = wire::Teaching {
        code: "x".into(),
        outcome: Some(wire::Outcome::Unclear(wire::Empty {})),
        ..Default::default()
    };
    assert!(Teaching::decode(&no_whose.encode_to_vec()).is_err_and(|e| e.starts_with("teaching-whose-unknown")));
    let new_class = wire::Teaching { outcome: Some(wire::Outcome::Refused(3)), ..no_whose.clone() };
    assert!(Teaching::decode(&new_class.encode_to_vec()).is_err_and(|e| e.starts_with("teaching-retry-unknown")));
    assert!(Teaching::decode(b"\xff\xff").is_err_and(|e| e.starts_with("teaching-malformed")));
    // K m/30467: a teaching with no code can't be matched, so it's refused, never read with an empty one.
    let codeless = wire::Teaching { code: String::new(), ..no_whose.clone() };
    let codeless =
        wire::Teaching { whose: Some(wire::Whose { who: Some(wire::Who::Caller(wire::Empty {})) }), ..codeless };
    assert!(Teaching::decode(&codeless.encode_to_vec()).is_err_and(|e| e.starts_with("teaching-code-empty")));
}

/// K m/29830: informs ride trailing metadata as one encoded Informs, so no product's proto changes to carry them.
#[test]
fn informs_round_trip_through_their_metadata_value() {
    let informs = vec![
        Inform::new(Kinds::Detached, "run 7 is detached", vec!["process:7".into()]),
        Inform::new(Kinds::Detached, "assess gap g", vec!["gap:g".into()]).owed("assess:g", "a verdict on gap:g"),
    ];
    assert_eq!(decode_informs(&encode_informs(&informs)), Ok(informs));
    assert_eq!(decode_informs(&encode_informs(&[])), Ok(Vec::new()));
    assert!(decode_informs(b"\xff\xff").is_err_and(|e| e.starts_with("informs-malformed")));
}

/// D m/30464: a teaching rides a Status's details as its one google.rpc.Status detail, and reads back; details with no
/// teaching are None; a teaching this build can't read is Unreadable, said, never failing the call, whose code and
/// message stand on their own.
#[test]
fn a_teaching_rides_a_status_and_one_that_cant_be_read_is_set_aside() {
    let refused = Teaching::refused(
        "carried-write-unbound",
        "a carried call reaches reads only",
        RetryClass::Blanket,
        Whose::Caller,
    );
    let details = status_details(7, &refused.message(), &refused);
    assert_eq!(teaching_in(&details), InDetails::Taught(refused.clone()));
    let status = wire::RpcStatus::decode(details.as_slice()).unwrap();
    assert_eq!((status.code, status.message.as_str()), (7, "carried-write-unbound: a carried call reaches reads only"));

    let other = wire::RpcStatus {
        code: 7,
        message: "m".into(),
        details: vec![prost_types::Any { type_url: "type.googleapis.com/google.rpc.ErrorInfo".into(), value: vec![] }],
    };
    assert_eq!(teaching_in(&other.encode_to_vec()), InDetails::None, "another detail is no teaching");
    assert_eq!(teaching_in(b""), InDetails::None, "no details at all");
    // E m/30763: bytes that aren't a Status are a broken or foreign sender, said as such, never read as an old one.
    assert!(
        matches!(teaching_in(b"\xff\xff\xff"), InDetails::Unreadable(why) if why.starts_with("status-details-malformed")),
        "details that aren't a Status"
    );

    let newer = wire::RpcStatus {
        code: 7,
        message: "m".into(),
        details: vec![prost_types::Any {
            type_url: TEACHING_TYPE_URL.into(),
            value: wire::Teaching { code: "x".into(), ..Default::default() }.encode_to_vec(),
        }],
    };
    assert!(
        matches!(teaching_in(&newer.encode_to_vec()), InDetails::Unreadable(why) if why.starts_with("teaching-outcome-unknown"))
    );
}
