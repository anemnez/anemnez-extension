//! The fixture against the contract's own check, and over a real socket.

use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::descriptor::check_descriptor;
use anemnez_extension::options::{ResultShape, Route};
use anemnez_extension::principal::Token;

use crate::{DIGEST_KEY, Variant, pb, serve};

/// The good descriptor offers every method shape, and Ping, with no anemnez options, is not exposed.
#[test]
fn the_good_fixture_offers_every_method_shape() {
    let methods = check_descriptor(Variant::Good.file_descriptor_set()).expect("admitted");
    let paths: Vec<_> = methods
        .iter()
        .map(|m| m.path.rsplit('/').next().unwrap_or_default())
        .collect();
    assert_eq!(
        paths,
        ["Items", "Recent", "Get", "Hold", "Run", "Wait"],
        "Ping is not exposed"
    );
    let by = |name: &str| methods.iter().find(|m| m.path.ends_with(name)).expect(name);
    assert_eq!(by("Items").options.result, ResultShape::Cursor);
    assert_eq!(by("Items").options.route, Route::Any);
    assert_eq!(by("Recent").options.result, ResultShape::Bound(5));
    let hold = by("Hold");
    assert!(hold.options.blocks);
    assert_eq!(
        hold.options.route,
        Route::Keyed {
            group: "fixture-repo".into(),
            key: "repo".into()
        }
    );
    assert_eq!(hold.route_key.as_ref().map(|k| k.number), Some(1));
    assert!(by("Run").options.runs_to_completion);
    assert!(
        !by("Wait").options.runs_to_completion,
        "Wait owns its budget by its field, not the flag"
    );
    assert_eq!(
        by("Items").options.examples,
        [anemnez_extension::options::Example::new(
            r#"{"cursor": ""}"#,
            "The first page"
        )]
    );
}

/// Each refusing variant is refused by the contract's check where the rule is the contract's, and
/// admitted by it where the rule is the bridge's own (version range, names), for the bridge to refuse.
#[test]
fn each_variant_is_refused_where_its_rule_lives() {
    let names = |v: Variant| match check_descriptor(v.file_descriptor_set()) {
        Ok(_) => Vec::new(),
        Err(refused) => refused.iter().map(|r| r.name()).collect(),
    };
    assert_eq!(names(Variant::Unbounded), ["unbounded-collection"]);
    assert_eq!(names(Variant::PersonActAllow), ["person-act-allowed"]);
    for bridges in [
        Variant::WrongMajor,
        Variant::CollidingNames,
        Variant::ReservedName,
    ] {
        assert!(
            names(bridges).is_empty(),
            "{bridges:?} is the bridge's to refuse"
        );
    }
    assert_eq!(
        (
            Variant::WrongMajor.contract(),
            Variant::WrongMajor.version()
        ),
        ("fixture", "2.0.0")
    );
    assert_eq!(Variant::ReservedName.contract(), "string");
    let colliding =
        check_descriptor(Variant::CollidingNames.file_descriptor_set()).expect("admitted");
    let methods: Vec<_> = colliding
        .iter()
        .map(|m| m.path.rsplit('/').next().unwrap_or_default())
        .collect();
    assert_eq!(methods, ["GetURL", "GetUrl"]);
}

fn with_token<T>(message: T, token: &str) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(
        anemnez_extension::principal::GRPC_METADATA_KEY,
        token.parse().expect("ascii"),
    );
    request
}

/// Over the fixture's real socket: every method answers its shape, a carried token's digest comes
/// back, and Describe, Health and Events are the contract's.
#[tokio::test(flavor = "multi_thread")]
async fn the_good_fixture_answers_over_its_socket() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), manual.clock(), Variant::Good)
        .await
        .expect("serves");
    let channel = running.channel().await.expect("connects");
    let mut fixture = pb::fixture_client::FixtureClient::new(channel.clone());

    // Items pages to the end.
    let (mut seen, mut cursor) = (Vec::new(), String::new());
    loop {
        let page = fixture
            .items(pb::ItemsRequest { cursor })
            .await
            .expect("a page")
            .into_inner();
        assert!(page.items.len() <= 5);
        seen.extend(page.items.into_iter().map(|i| i.id));
        if page.next.is_empty() {
            break;
        }
        cursor = page.next;
    }
    assert_eq!(seen.len(), 12);

    let recent = fixture
        .recent(pb::RecentRequest {})
        .await
        .expect("recent")
        .into_inner();
    assert_eq!(recent.items.len(), 5);
    let got = fixture
        .get(pb::GetRequest { id: "i03".into() })
        .await
        .expect("got")
        .into_inner();
    assert_eq!(got.name, "item 3");
    let missing = fixture
        .get(pb::GetRequest { id: "nope".into() })
        .await
        .expect_err("missing");
    assert_eq!(missing.code(), tonic::Code::NotFound);

    // A token rides the call, and its digest comes back.
    let held = fixture
        .hold(with_token(
            pb::HoldRequest {
                repo: "agora".into(),
                why: "test".into(),
            },
            "token-abc",
        ))
        .await
        .expect("held");
    let echoed = held
        .metadata()
        .get(DIGEST_KEY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    assert_eq!(echoed, Some(Token::digest("token-abc")));
    let no_key = fixture
        .hold(pb::HoldRequest::default())
        .await
        .expect_err("no key");
    assert_eq!(
        (no_key.code(), no_key.message()),
        (tonic::Code::InvalidArgument, "no-key")
    );

    fixture.run(pb::RunRequest {}).await.expect("ran");
    fixture.ping(pb::PingRequest {}).await.expect("pong");

    // Wait owns its budget on the fixture's clock: it answers only once that clock has moved.
    let mut waiting = fixture.clone();
    let wait =
        tokio::spawn(async move { waiting.wait(pb::WaitRequest { timeout_ms: 30_000 }).await });
    assert!(
        manual.until_waiting(1, Duration::from_secs(30)).await,
        "the wait parks on the clock"
    );
    assert!(!wait.is_finished());
    manual.advance(Duration::from_secs(30)).await;
    assert_eq!(
        wait.await
            .expect("joins")
            .expect("waited")
            .into_inner()
            .waited_ms,
        30_000
    );

    assert_eq!(running.contract, "fixture");
}

/// The contract's own services on the fixture: Describe serves the variant's descriptor, Health is
/// state with a cursor, and Events after that cursor carries the next hold.
#[tokio::test(flavor = "multi_thread")]
async fn the_fixture_serves_the_contracts_own_services() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), manual.clock(), Variant::Good)
        .await
        .expect("serves");
    let channel = running.channel().await.expect("connects");
    let mut fixture = pb::fixture_client::FixtureClient::new(channel.clone());
    // The contract's own services.
    use anemnez_extension_serve::pb as contract;
    let described = contract::describe_client::DescribeClient::new(channel.clone())
        .get(contract::DescribeRequest {})
        .await
        .expect("described")
        .into_inner();
    assert_eq!(
        described.file_descriptor_set,
        Variant::Good.file_descriptor_set()
    );
    let health = contract::health_client::HealthClient::new(channel.clone())
        .get(contract::HealthRequest {})
        .await
        .expect("health")
        .into_inner();
    assert_eq!(health.state, contract::State::Serving as i32);
    let mut events = contract::events_client::EventsClient::new(channel)
        .events(contract::EventsRequest {
            after: health.as_of,
            kinds: vec!["held".into()],
        })
        .await
        .expect("opens")
        .into_inner();
    // Subscribed after Health's as_of, so the next hold is the first event it sees.
    fixture
        .hold(pb::HoldRequest {
            repo: "agora".into(),
            why: "after as_of".into(),
        })
        .await
        .expect("held again");
    let event = events
        .message()
        .await
        .expect("reads")
        .expect("the hold's event");
    assert_eq!(
        (event.kind.as_str(), event.payload),
        ("held", b"agora".to_vec())
    );
}
