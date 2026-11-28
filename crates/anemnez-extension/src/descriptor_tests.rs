//! check_descriptor's rows: every refusal by name, and a good descriptor's options as written.

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

fn method(name: &str, input: &str, output: &str, options: Option<&wire::MethodOptions>) -> Vec<u8> {
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

/// A map field is known by its entry's `map_entry` option, never by its name: a hand-written
/// message named `…Entry` is a list of messages (D).
#[test]
fn a_map_field_is_known_by_its_entrys_map_entry() {
    let mut request = message(
        "MakeRequest",
        &[
            ("labels", 1, Type::Message, Label::Repeated),
            ("rows", 2, Type::Message, Label::Repeated),
            ("name", 3, Type::String, Label::Optional),
        ],
    );
    request.field[0].type_name = Some(".pkg.MakeRequest.LabelsEntry".into());
    request.field[1].type_name = Some(".pkg.MakeRequest.RowEntry".into());
    let mut labels = message("LabelsEntry", &[]);
    labels.options = Some(prost_types::MessageOptions { map_entry: Some(true), ..Default::default() });
    request.nested_type = vec![labels, message("RowEntry", &[])];
    let maps: Vec<(String, bool)> = super::fields_of(Some(&request)).into_iter().map(|f| (f.name, f.map)).collect();
    assert_eq!(maps, [("labels".into(), true), ("rows".into(), false), ("name".into(), false)]);
}

fn set(methods: Vec<Vec<u8>>) -> Vec<u8> {
    let mut service = ServiceDescriptorProto { name: Some("Herald".into()), ..Default::default() }.encode_to_vec();
    for m in methods {
        service.extend(ld(2, &m));
    }
    let mut file = FileDescriptorProto {
        name: Some("herald.proto".into()),
        package: Some("herald.v2".into()),
        message_type: vec![
            message(
                "LandRequest",
                &[("train", 1, Type::String, Label::Optional), ("repo", 3, Type::String, Label::Optional)],
            ),
            message("LandReply", &[]),
            message("QueueReply", &[("trains", 1, Type::String, Label::Repeated)]),
            message("OrientRequest", &[]),
        ],
        ..Default::default()
    }
    .encode_to_vec();
    file.extend(ld(6, &service));
    ld(1, &file)
}

/// One file with one service per entry: its name and, when given, its `anemnez_service`.
fn services(entries: &[(&str, Option<&wire::ServiceOptions>)]) -> Vec<u8> {
    let mut file = FileDescriptorProto {
        name: Some("herald.proto".into()),
        package: Some("herald.v2".into()),
        ..Default::default()
    }
    .encode_to_vec();
    for (name, options) in entries {
        let mut service = ServiceDescriptorProto { name: Some((*name).into()), ..Default::default() }.encode_to_vec();
        if let Some(o) = options {
            service.extend(ld(3, &ld(SERVICE_OPTIONS_FIELD, &o.encode_to_vec())));
        }
        file.extend(ld(6, &service));
    }
    ld(1, &file)
}

fn herald() -> wire::ServiceOptions {
    wire::ServiceOptions {
        contract: "herald".into(),
        version: "2.3.0".into(),
        cursor_scope: 2,
        parts_depth: 1,
        events_kept: 64,
        guidance: String::new(),
    }
}

/// A bridge reads what a descriptor's one service declares of itself, with no manifest.
#[test]
fn service_of_reads_the_one_service_that_declares_itself() {
    let options =
        service_of(&services(&[("Admin", None), ("Herald", Some(&herald()))])).expect("one service declares itself");
    assert_eq!(options.contract, "herald");
    assert_eq!(options.version, "2.3.0");
    assert_eq!(options.cursor_scope, crate::options::CursorScope::Instance);
    assert_eq!((options.parts_depth, options.events_kept), (1, 64));
}

/// D: a raw service list that doesn't match the decoded one is malformed, naming the file,
/// never a silent truncation answered as service-options-missing.
#[test]
fn a_service_list_that_does_not_match_is_malformed() {
    let service = |name: &str| ServiceDescriptorProto { name: Some(name.into()), ..Default::default() };
    let file = FileDescriptorProto {
        name: Some("herald.proto".into()),
        service: vec![service("Herald"), service("Admin")],
        ..Default::default()
    };
    // The raw file carries one service where two were decoded: truncated.
    let raw = ld(6, &service("Herald").encode_to_vec());
    let refused = services_of(&file, &raw).expect_err("refused");
    assert_eq!(refused.name(), "malformed");
    assert!(refused.to_string().contains("herald.proto"), "{refused}");
}

/// A broken option is not a missing one: `anemnez_service` that can't be read (here, sent as a
/// varint, which protobuf itself would skip as an unknown field) is malformed, naming the service.
#[test]
fn an_unreadable_service_option_is_malformed_not_missing() {
    let mut file = FileDescriptorProto {
        name: Some("herald.proto".into()),
        package: Some("herald.v2".into()),
        ..Default::default()
    }
    .encode_to_vec();
    let mut service = ServiceDescriptorProto { name: Some("Herald".into()), ..Default::default() }.encode_to_vec();
    let mut varint = Vec::new();
    prost::encoding::encode_key(SERVICE_OPTIONS_FIELD, prost::encoding::WireType::Varint, &mut varint);
    varint.push(1);
    service.extend(ld(3, &varint));
    file.extend(ld(6, &service));
    assert_eq!(
        service_of(&ld(1, &file)).expect_err("refused"),
        DescriptorRefusal::ServiceOptionsMalformed("herald.v2.Herald".into())
    );
}

/// The same on a method: an `anemnez_method` that can't be read is options-malformed by path,
/// never 'not offered'.
#[test]
fn an_unreadable_method_option_is_malformed_not_unoffered() {
    let mut land = method("Land", ".herald.v2.LandRequest", ".herald.v2.LandReply", None);
    let mut varint = Vec::new();
    prost::encoding::encode_key(METHOD_OPTIONS_FIELD, prost::encoding::WireType::Varint, &mut varint);
    varint.push(1);
    land.extend(ld(4, &varint));
    let refused = check_descriptor(&set(vec![land])).expect_err("refused");
    assert_eq!(refused, vec![DescriptorRefusal::OptionsMalformed("/herald.v2.Herald/Land".into())]);
}

/// Every refusal names what is wrong: none declares, two declare, or one says too little.
#[test]
fn service_of_refuses_by_name() {
    let name = |r: Result<crate::options::ServiceOptions, DescriptorRefusal>| r.expect_err("refused").name();
    assert_eq!(name(service_of(&services(&[("Herald", None)]))), "service-options-missing");
    assert_eq!(
        service_of(&services(&[("Herald", Some(&herald())), ("Other", Some(&herald()))])).expect_err("refused"),
        DescriptorRefusal::ServiceOptionsTwice("herald.v2.Herald".into(), "herald.v2.Other".into())
    );
    let mut nameless = herald();
    nameless.contract.clear();
    assert_eq!(name(service_of(&services(&[("Herald", Some(&nameless))]))), "contract-missing");
    let mut unversioned = herald();
    unversioned.version.clear();
    assert_eq!(name(service_of(&services(&[("Herald", Some(&unversioned))]))), "version-missing");
    let mut scopeless = herald();
    scopeless.cursor_scope = 0;
    assert_eq!(name(service_of(&services(&[("Herald", Some(&scopeless))]))), "cursor-scope-missing");
    assert_eq!(name(service_of(b"not a descriptor")), "malformed");
}

fn good() -> wire::MethodOptions {
    wire::MethodOptions {
        description: "Land a train on its repo's main.".into(),
        result: Some(wire::Result { shape: Some(wire::Shape::One(wire::Empty {})) }),
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
        ask_within_secs: None,
        orient: false,
        bound_secs: None,
        causes: Vec::new(),
        repeat: None,
    }
}

/// A good orient (ORIENTATION.md `#orient-is-one-bounded-read`): a unary agent read with no argument, its
/// result bound at the most an orient may state.
fn orient() -> wire::MethodOptions {
    wire::MethodOptions {
        description: "What matters to you in Herald now: your trains and the approvals they wait on.".into(),
        result: Some(wire::Result { shape: Some(wire::Shape::Bound(1)) }),
        effect: 1,
        orient: true,
        ..good()
    }
}

/// guidance-is-a-service-option: a product's guidance is read whole from its one service, its first line is its
/// summary, and both bounds are admitted exactly and refused one past, by name, saying the length.
#[test]
fn guidance_is_read_with_its_summary_and_refused_past_its_bounds() {
    let mut said = herald();
    said.guidance = "Herald lands trains and releases them.\nAsk `herald waits <repo>` what a train lacks.".into();
    let read = service_of(&services(&[("Herald", Some(&said))])).expect("admitted");
    assert_eq!(read.guidance, said.guidance);
    assert_eq!(read.summary(), "Herald lands trains and releases them.");
    assert_eq!(service_of(&services(&[("Herald", Some(&herald()))])).expect("none yet").summary(), "");

    let summary = "s".repeat(crate::options::GUIDANCE_SUMMARY_MAX_CHARS);
    let body = "b".repeat(crate::options::GUIDANCE_MAX_CHARS - summary.len() - 1);
    let mut full = herald();
    full.guidance = format!("{summary}\n{body}");
    assert_eq!(full.guidance.chars().count(), crate::options::GUIDANCE_MAX_CHARS);
    service_of(&services(&[("Herald", Some(&full))])).expect("both bounds, exactly");

    let mut long = herald();
    long.guidance = format!("{summary}\n{body}b");
    let refused = service_of(&services(&[("Herald", Some(&long))])).expect_err("one past");
    assert_eq!(refused, DescriptorRefusal::GuidanceTooLong("herald.v2.Herald".into(), 1_201));
    assert!(refused.to_string().contains("1201 characters, past 1200"), "{refused}");

    let mut headed = herald();
    headed.guidance = format!("{summary}s\nthe rest");
    let refused = service_of(&services(&[("Herald", Some(&headed))])).expect_err("a summary one past");
    assert_eq!(refused, DescriptorRefusal::GuidanceSummaryTooLong("herald.v2.Herald".into(), 161));
    // Characters, never bytes: a multi-byte summary at the bound is admitted.
    let mut wide = herald();
    wide.guidance = "é".repeat(crate::options::GUIDANCE_SUMMARY_MAX_CHARS);
    service_of(&services(&[("Herald", Some(&wide))])).expect("160 characters, 320 bytes");
}

/// orient-is-one-bounded-read: one orient is admitted and read as such; a second, a write, one taking input,
/// one unbounded or bound to more than one reply, one for products, and one that streams are each refused by name.
#[test]
fn an_orient_is_one_unary_agent_read_with_no_input_and_a_bound() {
    let empty = ".herald.v2.OrientRequest";
    let reply = ".herald.v2.LandReply";
    let methods = check_descriptor(&set(vec![
        method("Orient", empty, reply, Some(&orient())),
        method("Land", ".herald.v2.LandRequest", reply, Some(&good())),
    ]))
    .expect("admitted");
    let marked: Vec<_> = methods.iter().map(|m| (m.path.as_str(), m.options.orient)).collect();
    assert_eq!(marked, [("/herald.v2.Herald/Orient", true), ("/herald.v2.Herald/Land", false)]);

    let refused = |options: wire::MethodOptions, input: &str| {
        check_descriptor(&set(vec![method("Orient", input, reply, Some(&options))]))
            .expect_err("refused")
            .iter()
            .map(DescriptorRefusal::name)
            .collect::<Vec<_>>()
    };
    assert_eq!(refused(wire::MethodOptions { effect: 2, ..orient() }, empty), ["orient-not-a-read"]);
    assert_eq!(refused(orient(), ".herald.v2.LandRequest"), ["orient-takes-input"]);
    // A count past one is many replies, never a longer text (K m/31487): 2 is refused as 1,500 always was.
    let past = Some(wire::Result { shape: Some(wire::Shape::Bound(2)) });
    assert_eq!(refused(wire::MethodOptions { result: past, ..orient() }, empty), ["orient-unbounded"]);
    let one = Some(wire::Result { shape: Some(wire::Shape::One(wire::Empty {})) });
    assert_eq!(refused(wire::MethodOptions { result: one, ..orient() }, empty), ["orient-unbounded"]);
    assert_eq!(refused(wire::MethodOptions { callers: 2, ..orient() }, empty), ["orient-not-for-agents"]);

    let twice = check_descriptor(&set(vec![
        method("Orient", empty, reply, Some(&orient())),
        method("Now", empty, reply, Some(&orient())),
    ]))
    .expect_err("two orients");
    assert_eq!(
        twice,
        [DescriptorRefusal::OrientTwice("/herald.v2.Herald/Orient".into(), "/herald.v2.Herald/Now".into())]
    );

    let mut streaming = MethodDescriptorProto {
        name: Some("Orient".into()),
        input_type: Some(empty.into()),
        output_type: Some(reply.into()),
        server_streaming: Some(true),
        ..Default::default()
    }
    .encode_to_vec();
    streaming.extend(ld(4, &ld(METHOD_OPTIONS_FIELD, &orient().encode_to_vec())));
    let refused = check_descriptor(&set(vec![streaming])).expect_err("a stream");
    assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), ["orient-streams"]);
}

/// an-ask-is-bounded-in-the-call: absent reads 45 and never 0, an explicit 0 stays 0 (it never asks),
/// the cap is admitted, and one past it refuses the descriptor by name.
#[test]
fn an_ask_bound_reads_45_when_absent_and_is_capped_at_120() {
    let mut never = good();
    never.ask_within_secs = Some(0);
    let mut capped = good();
    capped.ask_within_secs = Some(crate::options::ASK_WITHIN_CAP_SECS);
    let bytes = set(vec![
        method("Absent", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&good())),
        method("Never", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&never)),
        method("Capped", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&capped)),
    ]);
    let methods = check_descriptor(&bytes).expect("admitted");
    let bounds: Vec<_> = methods.iter().map(|m| (m.options.ask_within_secs, m.options.ask_within())).collect();
    assert_eq!(bounds, [(None, 45), (Some(0), 0), (Some(120), 120)]);

    let mut over = good();
    over.ask_within_secs = Some(crate::options::ASK_WITHIN_CAP_SECS + 1);
    let bytes = set(vec![method("Over", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&over))]);
    let refused = check_descriptor(&bytes).expect_err("past the cap");
    assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), ["ask-bound-over-cap"]);
    assert!(refused[0].to_string().contains("121"), "the value is said: {}", refused[0]);
}

/// LONG-JOBS §2 (D m/35312): a method's bound is read as stated, absent stays absent, the cap is admitted, and 0
/// or one past the cap refuses the descriptor by name, saying the value and the cap.
#[test]
fn a_work_bound_is_read_as_stated_and_refused_at_zero_and_past_its_cap() {
    let with = |secs: Option<u32>| {
        let mut options = good();
        options.bound_secs = secs;
        options
    };
    let bytes = set(vec![
        method("Absent", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&good())),
        method("Boot", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(Some(120)))),
        method("Capped", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(Some(895)))),
    ]);
    let methods = check_descriptor(&bytes).expect("admitted");
    let bounds: Vec<_> = methods.iter().map(|m| m.options.bound()).collect();
    assert_eq!(bounds, [None, Some(std::time::Duration::from_secs(120)), Some(std::time::Duration::from_secs(895))]);

    for (secs, name, said) in [(0, "bound-zero", "bound_secs 0"), (896, "bound-over-cap", "896 is past the cap of 895")]
    {
        let bytes = set(vec![method("Bad", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(Some(secs))))]);
        let refused = check_descriptor(&bytes).expect_err("refused");
        assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), [name]);
        assert!(refused[0].to_string().contains(said), "{}", refused[0]);
    }
}

/// D m/44391 (1), E m/44479: an Ask is held inside the method's work, so it ends ASK_MARGIN_SECS inside the bound.
/// Equal is refused ask-past-bound with both values, as is one inside by less than the margin; a stated 0 never asks;
/// an absent ask reads its default inside the bound, and a bound with no room for one is refused bound-holds-no-ask.
#[test]
fn an_ask_bound_past_the_work_bound_is_refused() {
    let with = |ask: Option<u32>, bound: u32| {
        let mut options = good();
        options.ask_within_secs = ask;
        options.bound_secs = Some(bound);
        options
    };
    let bytes = set(vec![
        method("Inside", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(Some(55), 60))),
        method("Unsaid", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(None, 30))),
        method("Never", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(Some(0), 3))),
        method("Long", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(None, 120))),
    ]);
    let admitted = check_descriptor(&bytes).expect("admitted");
    // D m/44447, E m/44485: an unsaid ask reads its default, ending the margin inside the stated bound.
    let asks: Vec<_> = admitted.iter().map(|m| m.options.ask_within()).collect();
    assert_eq!(asks, [55, 25, 0, 45]);
    for (ask, bound, name, said) in [
        (Some(60), 60, "ask-past-bound", "ask_within_secs 60"),
        (Some(56), 60, "ask-past-bound", "ask_within_secs 56"),
        (None, 5, "bound-holds-no-ask", "bound_secs 5"),
    ] {
        let bytes = set(vec![method("Bad", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&with(ask, bound)))]);
        let refused = check_descriptor(&bytes).expect_err("refused");
        assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), [name]);
        assert!(refused[0].to_string().contains(said), "{}", refused[0]);
    }
}

/// every-bridge-refuses-a-descriptor-alike: a good descriptor answers every offered method with
/// its options, a keyed route's field number, and skips a method that carries no anemnez options.
#[test]
fn a_good_descriptor_answers_every_offered_method() {
    let mut land = good();
    land.blocks = true;
    land.examples = vec![wire::WireExample { args_json: r#"{"repo": "agora"}"#.into(), caption: "Land agora".into() }];
    land.route = Some(wire::Route {
        to: Some(wire::To::Keyed(wire::Leader { group: "herald-repo".into(), key: "repo".into() })),
    });
    let mut queue = good();
    queue.effect = 1;
    queue.result = Some(wire::Result { shape: Some(wire::Shape::Cursor(wire::Empty {})) });
    queue.route = Some(wire::Route { to: Some(wire::To::Any(wire::Empty {})) });
    let bytes = set(vec![
        method("Land", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&land)),
        method("Queue", ".herald.v2.LandRequest", ".herald.v2.QueueReply", Some(&queue)),
        method("Internal", ".herald.v2.LandRequest", ".herald.v2.LandReply", None),
    ]);
    let methods = check_descriptor(&bytes).expect("admitted");
    let paths: Vec<_> = methods.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["/herald.v2.Herald/Land", "/herald.v2.Herald/Queue"], "Internal has no face");
    let input: Vec<_> =
        methods[0].input.iter().map(|f| (f.name.as_str(), f.number, f.kind.clone(), f.repeated)).collect();
    assert_eq!(
        input,
        [("train", 1, FieldKind::String, false), ("repo", 3, FieldKind::String, false)],
        "the request's fields come back, so no bridge re-parses the descriptor"
    );
    assert_eq!(methods[0].input_type, ".herald.v2.LandRequest");
    assert_eq!(methods[0].route_key, Some(RouteKey { name: "repo".into(), number: 3 }));
    assert_eq!(methods[0].options.route, Route::Keyed { group: "herald-repo".into(), key: "repo".into() });
    assert!(methods[0].options.blocks && !methods[0].options.runs_to_completion);
    assert_eq!(methods[0].options.examples, [crate::options::Example::new(r#"{"repo": "agora"}"#, "Land agora")]);
    assert_eq!(methods[1].options.result, ResultShape::Cursor);
    assert_eq!(methods[1].options.route, Route::Any);
}

/// D m/22425, m/22441, m/22509: `node` and `keyed_via` are read as declared, keyed_via's key checked at connect
/// like keyed's; a route whose variant this build doesn't know (a newer descriptor's, whose tag prost drops) reads
/// as Unknown, never local-first, and only no route at all is local-first.
#[test]
fn node_and_keyed_via_are_read_and_an_unknown_route_is_never_local_first() {
    use prost::Message as _;
    let mut node = good();
    node.route = Some(wire::Route { to: Some(wire::To::Node(wire::Empty {})) });
    let mut via = good();
    via.route = Some(wire::Route {
        to: Some(wire::To::KeyedVia(wire::KeyedVia {
            group: "substrate-mind".into(),
            key: "repo".into(),
            via: "residency".into(),
        })),
    });
    // A variant from a later build: field 7, an empty message. prost drops the tag, so `to` is unset.
    let later = wire::Route::decode(&[(7 << 3) | 2, 0][..]).expect("decodes");
    assert_eq!(later.to, None, "an unknown oneof tag is dropped");
    let mut newer = good();
    newer.route = Some(later);
    let mut absent = good();
    absent.route = None;
    let bytes = set(vec![
        method("Pinned", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&node)),
        method("Via", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&via)),
        method("Newer", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&newer)),
        method("Absent", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&absent)),
    ]);
    let methods = check_descriptor(&bytes).expect("admitted");
    let routes: Vec<_> = methods.iter().map(|m| m.options.route.clone()).collect();
    assert_eq!(
        routes,
        [
            Route::Node,
            Route::KeyedVia { group: "substrate-mind".into(), key: "repo".into(), via: "residency".into() },
            Route::Unknown,
            Route::LocalFirst,
        ]
    );
    assert_eq!(methods[1].route_key, Some(RouteKey { name: "repo".into(), number: 3 }), "keyed_via's key is checked");

    let mut bad = via.clone();
    bad.route = Some(wire::Route {
        to: Some(wire::To::KeyedVia(wire::KeyedVia { group: "g".into(), key: "nope".into(), via: "r".into() })),
    });
    let bytes = set(vec![method("Bad", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&bad))]);
    let refused = check_descriptor(&bytes).expect_err("no such field");
    assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), ["route-key"]);

    // E m/22586: an empty group or residency fact is refused at connect, never resolved as "".
    let unnamed = |to: wire::To| {
        let mut m = good();
        m.route = Some(wire::Route { to: Some(to) });
        let bytes = set(vec![method("Unnamed", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&m))]);
        let refused = check_descriptor(&bytes).expect_err("unnamed");
        (refused[0].name(), refused[0].to_string())
    };
    let kv = |group: &str, via: &str| {
        wire::To::KeyedVia(wire::KeyedVia { group: group.into(), key: "repo".into(), via: via.into() })
    };
    assert!(matches!(unnamed(kv("g", "")), ("route-unnamed", why) if why.ends_with("via is empty")));
    assert!(matches!(unnamed(kv("", "r")), ("route-unnamed", why) if why.ends_with("group is empty")));
    assert_eq!(unnamed(wire::To::Leader(String::new())).0, "route-unnamed");
    let keyed = wire::To::Keyed(wire::Leader { group: String::new(), key: "repo".into() });
    assert_eq!(unnamed(keyed).0, "route-unnamed");
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
        to: Some(wire::To::Keyed(wire::Leader { group: "herald-repo".into(), key: "nope".into() })),
    });
    let mut lost = keyed.clone();
    lost.route =
        Some(wire::Route { to: Some(wire::To::Keyed(wire::Leader { group: "g".into(), key: "repo".into() })) });
    let bytes = set(vec![
        method("Bare", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&bare)),
        method("Person", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&person)),
        method("Keyed", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&keyed)),
        method("Queue", ".herald.v2.LandRequest", ".herald.v2.QueueReply", Some(&good())),
        method("Lost", ".herald.v2.Missing", ".herald.v2.LandReply", Some(&lost)),
    ]);
    let refused = check_descriptor(&bytes).expect_err("refused");
    let named: Vec<(&str, String)> =
        refused.iter().map(|r| (r.name(), r.to_string().split(':').next().unwrap_or_default().to_owned())).collect();
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
    outer.nested_type.push(message("Inner", &[("repo", 7, Type::String, Label::Optional)]));
    let set = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            package: Some("p.v1".into()),
            message_type: vec![message("First", &[]), outer],
            ..Default::default()
        }],
    };
    assert_eq!(find_message(&set, ".p.v1.First").map(DescriptorProto::name), Some("First"));
    assert_eq!(find_message(&set, ".p.v1.Outer.Inner").map(DescriptorProto::name), Some("Inner"));
    assert_eq!(find_message(&set, ".p.v1.Inner"), None, "a nested type is not top-level");
    assert_eq!(find_message(&set, ".q.v1.First"), None, "another package");
}

/// An example must be a JSON object of the request's top-level fields, or the method is refused
/// example-mismatch by name; the full type check is the bridge's.
#[test]
fn an_example_that_is_not_the_requests_fields_is_refused() {
    let example = |args: &str| wire::WireExample { args_json: args.into(), caption: String::new() };
    let mut unknown = good();
    unknown.examples = vec![example(r#"{"repo": "agora", "nope": 1}"#)];
    let mut not_object = good();
    not_object.examples = vec![example(r#"["agora"]"#)];
    let mut not_json = good();
    not_json.examples = vec![example("herald.land{repo = 'agora'}")];
    let bytes = set(vec![
        method("Unknown", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&unknown)),
        method("NotObject", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&not_object)),
        method("NotJson", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&not_json)),
    ]);
    let refused = check_descriptor(&bytes).expect_err("refused");
    let found: Vec<String> = refused.iter().map(ToString::to_string).collect();
    assert_eq!(refused.iter().map(DescriptorRefusal::name).collect::<Vec<_>>(), ["example-mismatch"; 3]);
    assert!(found[0].contains("'nope' is not a field of LandRequest"), "{found:?}");
    assert!(found[1].contains("not a JSON object"), "{found:?}");
    assert!(found[2].contains("not JSON"), "{found:?}");
}

/// M m/31062: a descriptor's streaming methods, client, server and bidi, by their gRPC paths, and never a unary one;
/// bytes that aren't a descriptor are refused by name.
#[test]
fn streaming_paths_are_every_streaming_method_and_no_unary_one() {
    use prost::Message as _;
    use prost_types::{MethodDescriptorProto, ServiceDescriptorProto};
    let method = |name: &str, client: bool, server: bool| MethodDescriptorProto {
        name: Some(name.into()),
        client_streaming: Some(client),
        server_streaming: Some(server),
        ..MethodDescriptorProto::default()
    };
    let set = FileDescriptorSet {
        file: vec![FileDescriptorProto {
            package: Some("typhon.v1".into()),
            service: vec![ServiceDescriptorProto {
                name: Some("ModelRunner".into()),
                method: vec![
                    method("ListModels", false, false),
                    method("Subscribe", false, true),
                    method("Upload", true, false),
                    method("Chat", true, true),
                ],
                ..ServiceDescriptorProto::default()
            }],
            ..FileDescriptorProto::default()
        }],
    }
    .encode_to_vec();
    assert_eq!(
        super::streaming_paths(&set).expect("reads"),
        ["/typhon.v1.ModelRunner/Subscribe", "/typhon.v1.ModelRunner/Upload", "/typhon.v1.ModelRunner/Chat"]
    );
    assert!(matches!(super::streaming_paths(b"\xff\xff"), Err(DescriptorRefusal::Malformed(_))));
}

/// The registry rows (CONTRACT.md §1a): an entry built the way a producer's build writes one.
mod registry {
    use std::borrow::Cow;

    use hydramq_message::{Field, Message, Schema, Shape, Symbol, Value};

    use super::*;
    use crate::describe::Registry;
    use crate::described::check_described;

    fn s(t: &str) -> Value<'static> {
        Value::String(Cow::Owned(t.to_owned()))
    }

    fn sym(t: &str) -> Value<'static> {
        Value::Symbol(Symbol::Text(Cow::Owned(t.to_owned())))
    }

    fn st(fields: Vec<(&'static str, Value<'static>)>) -> Value<'static> {
        Value::Struct(fields.into_iter().map(|(k, v)| (Symbol::from(k), v)).collect())
    }

    /// A `train.landed` entry, its body a closed struct with one required string, `train`; `edit` changes its fields.
    fn entry(kind: &str, edit: impl FnOnce(&mut Vec<(&'static str, Value<'static>)>)) -> Vec<u8> {
        let shape = Shape::String { min_len: None, max_len: None };
        let body = Shape::Struct { fields: vec![Field { name: "train".into(), shape, required: true }], open: false };
        let schema = Schema { name: format!("herald.{kind}"), body }.to_message().body.expect("a schema's body");
        let example = st(vec![("caption", s("one train landed")), ("body", st(vec![("train", s("train/h-x"))]))]);
        let mut fields = vec![
            ("kind", s(kind)),
            ("schema", schema),
            ("version", Value::from(1i64)),
            ("description", s("A train landed on its repo's main; published once per landing.")),
            ("examples", Value::List(vec![example])),
            ("stability", sym("stable")),
            ("asks", s("none")),
        ];
        edit(&mut fields);
        let message = Message {
            kind: Some(Cow::Borrowed(hydramq_message::ENTRY_TYPE)),
            body: Some(st(fields)),
            ..Message::default()
        };
        hydramq_message::encode_message_canonical(&message).expect("encodes")
    }

    fn causing(causes: &[&str]) -> Vec<u8> {
        let options = wire::MethodOptions { causes: causes.iter().map(|c| (*c).to_owned()).collect(), ..good() };
        set(vec![method("Land", ".herald.v2.LandRequest", ".herald.v2.LandReply", Some(&options))])
    }

    fn names(refused: Vec<DescriptorRefusal>) -> Vec<&'static str> {
        let mut names: Vec<_> = refused.iter().map(DescriptorRefusal::name).collect();
        names.sort_unstable();
        names
    }

    /// CONTRACT.md a-registry-is-checked-as-the-descriptor-is: a declared kind and a method that causes it are admitted
    /// together, the kinds in the registry's order.
    #[test]
    fn a_registry_and_its_causes_are_admitted_together() {
        let registry = Registry::Declared(vec![entry("train.landed", |_| {}), entry("train.bounced", |_| {})]);
        let described = check_described(&causing(&["train.landed"]), &registry).expect("admitted");
        assert_eq!(described.kinds, ["train.landed", "train.bounced"]);
        assert!(described.declared);
        assert_eq!(described.methods[0].options.causes, ["train.landed"]);
    }

    /// CONTRACT.md a-registry-is-checked-as-the-descriptor-is: every refusal at once, each by name, the descriptor's
    /// own included.
    #[test]
    fn every_registry_refusal_is_said_at_once() {
        let other = hydramq_message::encode_message_canonical(&Message {
            kind: Some(Cow::Borrowed("herald.train.landed")),
            body: Some(st(vec![])),
            ..Message::default()
        })
        .expect("encodes");
        let registry = Registry::Declared(vec![
            b"not a message".to_vec(),
            other,
            entry("train.queued", |f| f.retain(|(k, _)| *k != "description")),
            entry("train.landed", |_| {}),
            entry("train.landed", |_| {}),
            entry("train.parked", |f| {
                f.retain(|(k, _)| *k != "stability");
                f.push(("stability", sym("deprecated")));
            }),
            entry("train.held", |f| f.push(("causes", Value::List(vec![s("/herald.v2.Herald/Land")])))),
            entry("train.bounced", |f| {
                f.retain(|(k, _)| *k != "examples");
                let wrong = st(vec![("caption", s("no train")), ("body", st(vec![("repo", s("ergon"))]))]);
                f.push(("examples", Value::List(vec![wrong])));
            }),
        ]);
        let refused = check_described(&causing(&["train.landed", "train.lost"]), &registry).expect_err("refused");
        assert_eq!(
            names(refused.clone()),
            [
                "cause-undeclared",
                "causes-written",
                "deprecated-without-replacement",
                "entry-invalid",
                "entry-not-an-entry",
                "entry-unread",
                "example-invalid",
                "kind-twice",
            ],
            "{refused:#?}"
        );
        let said: Vec<String> = refused.iter().map(ToString::to_string).collect();
        assert!(said.iter().any(|l| l.contains("\"train.lost\"") && l.contains("/herald.v2.Herald/Land")), "{said:?}");
        assert!(said.iter().any(|l| l.starts_with("train.queued: entry-invalid") && l.contains("description")));
        assert!(said.iter().any(|l| l.starts_with("train.bounced: example-invalid") && l.contains("no train")));
    }

    /// CONTRACT.md an-undeclared-registry-is-said: with no registry, a method that names a cause is refused, and one
    /// that names none is admitted as undeclared, never as "no events".
    #[test]
    fn with_no_registry_every_cause_is_undeclared() {
        let refused = check_described(&causing(&["train.landed"]), &Registry::Undeclared).expect_err("refused");
        assert_eq!(names(refused), ["cause-undeclared"]);
        let described = check_described(&causing(&[]), &Registry::Undeclared).expect("admitted");
        assert!(!described.declared && described.kinds.is_empty());
    }

    /// CONTRACT.md a-registry-is-byte-stable and §1a's causes: the registry served is the entries in their order, each
    /// entry's causes filled from the methods (an empty list for a kind none causes), the same bytes every time.
    #[test]
    fn the_served_registry_takes_its_causes_from_the_methods() {
        let entries = vec![entry("train.landed", |_| {}), entry("train.bounced", |_| {})];
        let served = crate::described::registry_of(&causing(&["train.landed"]), entries.clone()).expect("served");
        let table = hydramq_message::SymbolTable::system();
        let causes = |bytes: &[u8]| -> Option<Vec<String>> {
            let message = hydramq_message::decode_message(bytes, &hydramq_message::Limits::default(), &table)
                .expect("reads")
                .body
                .expect("a body");
            let Value::Struct(fields) = message else { panic!("a struct") };
            let (_, said) = fields.into_iter().find(|(k, _)| matches!(k, Symbol::Text(t) if t == "causes"))?;
            let Value::List(paths) = said else { return None };
            Some(paths.iter().map(|p| if let Value::String(p) = p { p.to_string() } else { String::new() }).collect())
        };
        assert_eq!(causes(&served.entries[0]), Some(vec!["/herald.v2.Herald/Land".to_owned()]));
        assert_eq!(causes(&served.entries[1]), Some(Vec::new()), "said empty, never absent");
        let again = crate::described::registry_of(&causing(&["train.landed"]), entries).expect("served");
        assert_eq!(again, served, "the same build serves the same bytes");
        let written = vec![entry("train.held", |f| f.push(("causes", Value::List(Vec::new()))))];
        let refused = crate::described::registry_of(&causing(&[]), written).expect_err("refused");
        assert_eq!(names(refused), ["causes-written"]);
    }

    /// D m/48108: what the serving face serves passes a reader's check and fails the built check (its causes are
    /// filled), and a served entry whose causes are absent or disagree with the methods is causes-mismatch.
    #[test]
    fn served_entries_are_judged_apart_from_built_ones() {
        let fds = causing(&["train.landed"]);
        let built = vec![entry("train.landed", |_| {}), entry("train.bounced", |_| {})];
        let served = crate::described::registry_of(&fds, built.clone()).expect("served").entries;
        let read =
            crate::described::check_served(&fds, &Registry::Declared(served.clone())).expect("a reader admits it");
        assert_eq!(read.kinds, ["train.landed", "train.bounced"]);
        let as_built = check_described(&fds, &Registry::Declared(served)).expect_err("served bytes aren't built ones");
        assert_eq!(names(as_built), ["causes-written", "causes-written"]);
        let wrong = vec![entry("train.landed", |f| f.push(("causes", Value::List(vec![s("/x.v1.X/Other")]))))];
        let refused = crate::described::check_served(&fds, &Registry::Declared(wrong)).expect_err("refused");
        assert_eq!(names(refused.clone()), ["causes-mismatch"]);
        assert!(refused[0].to_string().contains("/herald.v2.Herald/Land"), "{}", refused[0]);
        let absent = crate::described::check_served(&fds, &Registry::Declared(built)).expect_err("refused");
        assert_eq!(names(absent), ["causes-mismatch", "causes-mismatch"], "a served entry carries its causes");
    }

    /// CONTRACT.md's repeat option: same_by names a top-level scalar of the request, by number; a missing field is
    /// repeat-field; a variant this build doesn't know is repeat-unknown, never none; absent is none.
    #[test]
    fn a_repeat_names_a_scalar_of_the_request_or_is_refused() {
        let repeat = |how: Option<wire::How>| {
            let options = wire::MethodOptions { repeat: Some(wire::Repeat { how }), ..good() };
            check_descriptor(&set(vec![method(
                "Land",
                ".herald.v2.LandRequest",
                ".herald.v2.LandReply",
                Some(&options),
            )]))
        };
        let by = |field: &str| Some(wire::How::SameBy(wire::SameBy { field: field.into() }));
        let admitted = repeat(by("repo")).expect("admitted");
        assert_eq!(admitted[0].options.repeat, crate::options::Repeat::SameBy { field: "repo".into(), number: 3 });
        assert_eq!(names(repeat(by("missing")).expect_err("refused")), ["repeat-field"]);
        assert_eq!(names(repeat(None).expect_err("refused")), ["repeat-unknown"]);
        let none = repeat(Some(wire::How::None(wire::Empty {}))).expect("admitted");
        assert_eq!(none[0].options.repeat, crate::options::Repeat::None);
        let absent = check_descriptor(&causing(&[])).expect("admitted");
        assert_eq!(absent[0].options.repeat, crate::options::Repeat::None, "absent is none");
    }
}
