use serde_json::json;

use super::{
    EVENTS_PATH, EVENTS_TOOL, FlattenRefusal, Stamp, StampLease, ToolKind, flatten, instructions, op_name, resolve,
};
use crate::descriptor::{Field, FieldKind, Method};
use crate::options::{
    Callers, CursorScope, Effect, Example, MethodOptions, Permission, Presence, ResultShape, ServiceOptions, Session,
};

fn field(name: &str, kind: FieldKind) -> Field {
    Field { name: name.into(), number: 1, kind, repeated: false, optional: false, map: false }
}

/// A service keeping `events_kept` events.
fn service(events_kept: u32) -> ServiceOptions {
    ServiceOptions {
        contract: "pkg".into(),
        version: "1.0.0".into(),
        cursor_scope: CursorScope::Instance,
        parts_depth: 1,
        events_kept,
        guidance: String::new(),
    }
}

fn method(path: &str, options: MethodOptions, input: Vec<Field>) -> Method {
    let input_type = format!(".pkg.{}Request", path.rsplit('/').next().unwrap_or_default());
    Method {
        path: path.into(),
        options,
        route_key: None,
        client_streaming: false,
        server_streaming: false,
        input,
        input_type,
        output_type: ".pkg.Reply".into(),
        output_media: Vec::new(),
    }
}

fn agent(description: &str, effect: Effect, permission: Permission) -> MethodOptions {
    MethodOptions::new(description, ResultShape::One, effect, Presence::Full, permission, Callers::Agent)
}

/// CONTRACT.md one-mapping-flattens-a-descriptor: one tool per agent method, named in snake case,
/// with its description, examples, schema and the facts the host needs; never a product's, an
/// owner's or a hidden method; and the events tool last.
#[test]
fn one_mapping_flattens_a_descriptor() {
    let mut items = MethodOptions::new(
        "Every item, a page at a time.",
        ResultShape::Cursor,
        Effect::Read,
        Presence::Full,
        Permission::Allow,
        Callers::Agent,
    );
    items.examples = vec![Example::new(r#"{"cursor": ""}"#, "The first page")];
    let mut hold = agent("Hold a repo.", Effect::Write, Permission::Ask);
    hold.blocks = true;
    hold.person_act = true;
    let mut hidden = agent("Hidden.", Effect::Read, Permission::Allow);
    hidden.presence = Presence::Hidden;
    let mut open = agent("Open a session.", Effect::Write, Permission::Allow);
    open.session = Some(Session::Opens);
    let owner =
        MethodOptions::new("Owner.", ResultShape::One, Effect::Act, Presence::Full, Permission::Ask, Callers::Owner);
    let methods = [
        method("/pkg.S/ListItems", items, vec![field("cursor", FieldKind::String)]),
        method("/pkg.S/HoldRepo", hold, vec![field("repo", FieldKind::String), field("why", FieldKind::String)]),
        method("/pkg.S/Secret", hidden, vec![]),
        method("/pkg.S/OpenSession", open, vec![]),
        method("/pkg.S/Rotate", owner, vec![]),
    ];
    let tools = flatten(&service(64), &methods).expect("flattens");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["list_items", "hold_repo", "open_session", EVENTS_TOOL], "agent methods, then events");
    assert_eq!(tools[2].session, Some(Session::Opens), "a session's id is a field of its requests");
    assert_eq!(tools[2].to_json()["_meta"]["anemnez"]["session"], "OPENS");

    let list = &tools[0];
    assert_eq!((list.kind, list.path.as_str()), (ToolKind::Call, "/pkg.S/ListItems"));
    assert_eq!((list.effect, list.consent, list.long), (Effect::Read, Permission::Allow, false));
    assert_eq!(list.description, "Every item, a page at a time.\n\nExamples:\n- The first page: {\"cursor\": \"\"}");
    assert_eq!(
        list.input_schema,
        json!({
            "type": "object",
            "properties": { "cursor": { "type": "string" } },
            "additionalProperties": false,
        })
    );
    let hold = &tools[1];
    assert_eq!((hold.effect, hold.consent, hold.long), (Effect::Write, Permission::Ask, true), "blocks is long");

    let listed = list.to_json();
    assert_eq!(listed["name"], "list_items");
    assert_eq!(listed["inputSchema"], list.input_schema);
    assert_eq!(
        listed["annotations"],
        json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false })
    );
    assert_eq!(hold.to_json()["annotations"]["readOnlyHint"], false);
    assert_eq!(
        listed["_meta"]["anemnez"],
        json!({ "path": "/pkg.S/ListItems", "effect": "READ", "consent": "ALLOW", "long": false, "result": "cursor", "session": null })
    );
    assert_eq!((list.result, hold.result), (ResultShape::Cursor, ResultShape::One), "a cursor result is a page tool");
}

/// Each field kind in the proto3 JSON mapping, a repeated field as an array of it, and a map as an
/// object.
#[test]
fn each_field_kind_has_its_json_mapping() {
    let kinds = [
        ("d", FieldKind::Double, json!({ "type": "number" })),
        ("i", FieldKind::Int32, json!({ "type": "integer" })),
        ("u", FieldKind::Fixed32, json!({ "type": "integer" })),
        ("l", FieldKind::Int64, json!({ "type": ["integer", "string"] })),
        ("ul", FieldKind::Uint64, json!({ "type": ["integer", "string"] })),
        ("b", FieldKind::Bool, json!({ "type": "boolean" })),
        ("s", FieldKind::String, json!({ "type": "string" })),
        ("y", FieldKind::Bytes, json!({ "type": "string", "contentEncoding": "base64" })),
        (
            "e",
            FieldKind::Enum(".pkg.Color".into()),
            json!({ "type": ["string", "integer"], "description": "enum .pkg.Color" }),
        ),
        (
            "t",
            FieldKind::Message(".google.protobuf.Timestamp".into()),
            json!({ "type": "string", "format": "date-time" }),
        ),
        ("du", FieldKind::Message(".google.protobuf.Duration".into()), json!({ "type": "string" })),
        ("w", FieldKind::Message(".google.protobuf.Int32Value".into()), json!({ "type": "integer" })),
        ("st", FieldKind::Message(".google.protobuf.Struct".into()), json!({ "type": "object" })),
        ("m", FieldKind::Message(".pkg.Item".into()), json!({ "type": "object", "description": "message .pkg.Item" })),
    ];
    let mut input: Vec<Field> = kinds.iter().map(|(n, k, _)| field(n, k.clone())).collect();
    input.push(Field { repeated: true, ..field("ids", FieldKind::String) });
    input.push(Field {
        repeated: true,
        map: true,
        ..field("labels", FieldKind::Message(".pkg.MakeRequest.LabelsEntry".into()))
    });
    input.push(Field { repeated: true, ..field("rows", FieldKind::Message(".pkg.MakeRequest.RowEntry".into())) });
    let tools = flatten(&service(64), &[method("/pkg.S/Make", agent("Make.", Effect::Write, Permission::Ask), input)])
        .expect("flattens");
    let properties = &tools[0].input_schema["properties"];
    for (name, _, want) in &kinds {
        assert_eq!(&properties[name], want, "{name}");
    }
    assert_eq!(properties["ids"], json!({ "type": "array", "items": { "type": "string" } }));
    assert_eq!(properties["labels"], json!({ "type": "object" }), "a map is an object");
    assert_eq!(
        properties["rows"],
        json!({ "type": "array", "items": { "type": "object", "description": "message .pkg.MakeRequest.RowEntry" } }),
        "a message named …Entry that is not map_entry is a list"
    );
}

/// CONTRACT.md a-flattened-name-is-one-tool: names that flatten alike are refused naming both
/// paths, and a method flattening to `events` is refused; every refusal is answered.
#[test]
fn a_flattened_name_is_one_tool() {
    assert_eq!(op_name("/pkg.S/GetURL"), "get_url");
    assert_eq!(op_name("/pkg.S/GetUrl"), "get_url");
    assert_eq!(op_name("/pkg.S/URLParser"), "url_parser");
    assert_eq!(op_name("/pkg.S/Get2Things"), "get2_things");
    assert_eq!(op_name("/pkg.S/Ping"), "ping");
    // Substrate's mount's names, held so its switch to this one renames nothing (B).
    for (method, op) in [
        ("Queue", "queue"),
        ("ListOutcomes", "list_outcomes"),
        ("GetURL", "get_url"),
        ("URLFor", "url_for"),
        ("GetHTTPStatus", "get_http_status"),
        ("List_Items", "list_items"),
        ("V2Status", "v2_status"),
    ] {
        assert_eq!(op_name(&format!("/pkg.S/{method}")), op, "{method}");
    }
    let read = || agent("Read.", Effect::Read, Permission::Allow);
    let methods = [
        method("/pkg.S/GetURL", read(), vec![]),
        method("/pkg.S/GetUrl", read(), vec![]),
        method("/pkg.T/GetUrl", read(), vec![]),
        method("/pkg.S/Events", read(), vec![]),
    ];
    let refusals = flatten(&service(64), &methods).expect_err("refused");
    let named: Vec<(&str, &FlattenRefusal)> = refusals.iter().map(|r| (r.name(), r)).collect();
    assert_eq!(
        refusals,
        [
            FlattenRefusal::NameCollision("get_url".into(), "/pkg.S/GetURL".into(), "/pkg.S/GetUrl".into()),
            FlattenRefusal::NameCollision("get_url".into(), "/pkg.S/GetURL".into(), "/pkg.T/GetUrl".into()),
            FlattenRefusal::ReservedName("/pkg.S/Events".into(), EVENTS_TOOL.into()),
        ],
        "{named:?}"
    );
    assert_eq!(refusals[0].name(), "name-collision");
    assert_eq!(refusals[2].name(), "reserved-name");
    // Each says its own paths and name, so a face reads what to fix from the refusal's words (E m/41309).
    assert_eq!(refusals[0].to_string(), "/pkg.S/GetURL and /pkg.S/GetUrl: name-collision: both flatten to get_url");
    assert_eq!(
        refusals[2].to_string(),
        format!("/pkg.S/Events: reserved-name: flattens to {EVENTS_TOOL}, a reserved name")
    );
}

/// CONTRACT.md events-flatten-to-one-bounded-read: every face has the events tool, reading the
/// product's own Events stream.
#[test]
fn events_flatten_to_one_bounded_read() {
    assert!(flatten(&service(0), &[]).expect("flattens").is_empty(), "no events kept, no events tool");
    let tools = flatten(&service(64), &[]).expect("a descriptor keeping events has the events tool");
    assert_eq!(tools.len(), 1);
    let events = &tools[0];
    assert_eq!((events.name.as_str(), events.kind, events.path.as_str()), (EVENTS_TOOL, ToolKind::Events, EVENTS_PATH));
    assert_eq!((events.effect, events.consent, events.long), (Effect::Read, Permission::Allow, false));
    assert_eq!(
        events.input_schema,
        json!({
            "type": "object",
            "properties": {
                "after": { "type": "string" },
                "kinds": { "type": "array", "items": { "type": "string" } },
                "catalog": { "type": "boolean" },
                "kind": { "type": "string" },
            },
            "additionalProperties": false,
        })
    );
}

/// CONTRACT.md an-mcp-call-names-one-method: a call's name resolves to its tool, never a path the
/// client sends.
#[test]
fn an_mcp_call_names_one_method() {
    let tools = flatten(&service(64), &[method("/pkg.S/Get", agent("Get.", Effect::Read, Permission::Allow), vec![])])
        .expect("flattens");
    assert_eq!(resolve(&tools, "get").map(|t| t.path.as_str()), Some("/pkg.S/Get"));
    assert_eq!(resolve(&tools, EVENTS_TOOL).map(|t| t.kind), Some(ToolKind::Events));
    assert!(resolve(&tools, "/pkg.S/Get").is_none(), "a path is not a name");
    assert!(resolve(&tools, "Get").is_none());
    assert_eq!(super::NO_SUCH_TOOL, "no-such-tool");
}

/// CONTRACT.md an-external-caller-is-parsed-here-and-trusted-only-from-its-peer: the stamp Agora
/// writes reads back as itself once vouched, over `_meta` and over metadata alike; anything that
/// is not Agora's shape reads as external with no lane, never a guess.
#[test]
fn an_external_caller_is_parsed_here_and_trusted_only_from_its_peer() {
    // The shape Agora's gateway writes today (agora gateway.rs caller_stamp).
    let agoras = json!({
        "class": "external",
        "place": { "cwd": "/Users/x/repo", "proven": true },
        "lane": "H",
        "leased": true,
        "lease": 7,
    });
    let stamp = Stamp::from_json(&agoras).0;
    let want = Stamp::new(Some("H".into()), Some("/Users/x/repo".into()), Some(StampLease::new(7, false)));
    assert_eq!(stamp, want);
    assert_eq!(stamp.to_json(), agoras, "one value on both faces");
    assert_eq!(Stamp::from_metadata(&want.to_metadata()).0, want);
    let lapsed = Stamp::new(Some("H".into()), None, Some(StampLease::new(9, true)));
    assert_eq!(
        lapsed.to_json(),
        json!({ "class": "external", "place": { "proven": false }, "lane": "H", "lease_lapsed": true, "lease": 9 })
    );
    assert_eq!(Stamp::from_json(&lapsed.to_json()).0, lapsed);

    let none = Stamp::unvouched();
    assert_eq!(none.to_json(), json!({ "class": "external", "place": { "proven": false } }));
    let reads = |v: serde_json::Value| Stamp::from_json(&v).0;
    for (why, value) in [
        ("not external", json!({ "class": "user", "lane": "H" })),
        ("no class", json!({ "lane": "H" })),
        ("not a plain label", json!({ "class": "external", "lane": "H\u{202E}" })),
        ("empty lane", json!({ "class": "external", "lane": "" })),
    ] {
        assert_eq!(reads(value).lane, None, "{why}");
    }
    let no_lane = reads(json!({ "class": "external", "leased": true, "lease": 7 }));
    assert_eq!(no_lane.lease, None, "a lease with no lane");
    assert_eq!(reads(json!({ "class": "external", "lane": "H", "leased": true, "lease": 0 })).lease, None, "token 0");
    assert_eq!(
        reads(json!({ "class": "external", "lane": "H", "leased": true, "lease_lapsed": true, "lease": 7 })).lease,
        None,
        "both leased and lapsed"
    );
    assert_eq!(
        reads(json!({ "class": "external", "place": { "cwd": "/x", "proven": false } })).cwd,
        None,
        "an unproven place"
    );
    assert_eq!(Stamp::from_metadata(b"not json").0, Stamp::unvouched());
    assert_eq!(super::STAMP_METADATA_KEY, "anemnez-caller-bin");
}

/// ORIENTATION.md §4: the orient tool says it is the orient under `_meta.anemnez`, and only it does, so every other
/// tool's listing stays byte for byte as it was; and a server's `instructions` are the product's guidance, whole,
/// or nothing when it has none yet.
#[test]
fn the_orient_tool_says_so_and_the_instructions_are_the_guidance() {
    let mut now = MethodOptions::new(
        "What matters to you now.",
        ResultShape::Bound(1),
        Effect::Read,
        Presence::Full,
        Permission::Allow,
        Callers::Agent,
    );
    now.orient = true;
    let methods = [
        method("/pkg.S/Orient", now, vec![]),
        method("/pkg.S/Get", agent("Get one.", Effect::Read, Permission::Allow), vec![]),
    ];
    let tools = flatten(&service(0), &methods).expect("flattens");
    assert!(tools[0].orient);
    assert_eq!(tools[0].to_json()["_meta"]["anemnez"]["orient"], true);
    assert!(!tools[1].orient);
    assert!(tools[1].to_json()["_meta"]["anemnez"].get("orient").is_none(), "said only on the orient");

    assert_eq!(instructions(&service(0)), None, "no guidance yet: nothing");
    let mut guided = service(0);
    guided.guidance = "Pkg keeps your items.\nEach tool answers its help.".into();
    assert_eq!(instructions(&guided), Some("Pkg keeps your items.\nEach tool answers its help."));
    assert_eq!(guided.summary(), "Pkg keeps your items.");
}

/// LONG-JOBS §2: a long tool carries its method's bound under `_meta.anemnez`, so a host bounds its job by it; a tool
/// whose method states none says nothing, so its listing stays byte for byte as it was.
#[test]
fn a_long_tool_carries_its_bound_and_an_unbounded_one_says_nothing() {
    let mut boot = agent("Boot a simulator.", Effect::Act, Permission::Allow);
    boot.blocks = true;
    boot.bound_secs = Some(120);
    let methods = [
        method("/pkg.S/Boot", boot, vec![]),
        method("/pkg.S/Get", agent("Get one.", Effect::Read, Permission::Allow), vec![]),
    ];
    let tools = flatten(&service(0), &methods).expect("flattens");
    assert_eq!((tools[0].long, tools[0].bound_secs), (true, Some(120)));
    assert_eq!(tools[0].to_json()["_meta"]["anemnez"]["bound_secs"], 120);
    assert_eq!(tools[1].bound_secs, None);
    assert!(tools[1].to_json()["_meta"]["anemnez"].get("bound_secs").is_none(), "said only when stated");
}
