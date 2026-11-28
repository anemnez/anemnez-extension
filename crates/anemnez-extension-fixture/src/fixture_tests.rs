//! The fixture against the contract's own check, and over a real socket.

use std::time::Duration;

use anemnez_clock::ManualClock;
use anemnez_extension::descriptor::{check_descriptor, service_of};
use anemnez_extension::options::{ResultShape, Route};
use anemnez_extension::principal::Token;

use crate::{DIGEST_KEY, Variant, pb, serve};

/// A bridge reads the service's own declaration from a descriptor protoc wrote, not only from
/// hand-built bytes (`descriptor::service_of`).
#[test]
fn the_fixture_declares_its_service() {
    let service = service_of(Variant::Good.file_descriptor_set()).expect("declared");
    assert_eq!((service.contract.as_str(), service.version.as_str()), ("fixture", "1.0.0"));
    assert_eq!(service.cursor_scope, anemnez_extension::options::CursorScope::Instance);
    assert_eq!(service.events_kept, 64);
}

/// The good fixture carries guidance (ORIENTATION.md §4), so a host's `instructions` are proven against a descriptor
/// protoc wrote: its summary is its first line, within the caps, and `mcp::instructions` answers it whole.
#[test]
fn the_fixture_carries_guidance_a_host_says_as_its_instructions() {
    use anemnez_extension::options::{GUIDANCE_MAX_CHARS, GUIDANCE_SUMMARY_MAX_CHARS};
    let service = service_of(Variant::Good.file_descriptor_set()).expect("declared");
    assert_eq!(service.summary(), "The fixture product: one of each method shape the contract describes.");
    assert!(service.summary().chars().count() <= GUIDANCE_SUMMARY_MAX_CHARS);
    assert!(service.guidance.chars().count() <= GUIDANCE_MAX_CHARS);
    assert_eq!(anemnez_extension::mcp::instructions(&service), Some(service.guidance.as_str()));
    assert!(service.guidance.contains("Recent is the newest five"), "{}", service.guidance);
}

/// The good fixture has an orient the descriptor checks admit (an agent read, no input, bound 1), so a host's orient
/// fan-out is proven against a native face: flattened, it alone carries the orient mark (D m/33316).
#[test]
fn the_good_fixtures_orient_is_admitted_and_alone_marked() {
    let methods = check_descriptor(Variant::Good.file_descriptor_set()).expect("admitted");
    let service = service_of(Variant::Good.file_descriptor_set()).expect("declared");
    let tools = anemnez_extension::mcp::flatten(&service, &methods).expect("flattened");
    let marked: Vec<String> = tools
        .iter()
        .map(anemnez_extension::mcp::Tool::to_json)
        .filter(|t| t["_meta"]["anemnez"]["orient"].as_bool() == Some(true))
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(marked.len(), 1, "{marked:?}");
    assert!(marked[0].ends_with("orient"), "{marked:?}");
}

/// The good descriptor offers every method shape, and Ping, with no anemnez options, is not exposed.
#[test]
fn the_good_fixture_offers_every_method_shape() {
    let methods = check_descriptor(Variant::Good.file_descriptor_set()).expect("admitted");
    let paths: Vec<_> = methods.iter().map(|m| m.path.rsplit('/').next().unwrap_or_default()).collect();
    assert_eq!(paths, ["Items", "Recent", "Get", "Hold", "Run", "Wait", "Snapshot", "Orient"], "Ping is not exposed");
    let by = |name: &str| methods.iter().find(|m| m.path.ends_with(name)).expect(name);
    assert_eq!(by("Items").options.result, ResultShape::Cursor);
    assert_eq!(by("Items").options.route, Route::Any);
    assert_eq!(by("Recent").options.result, ResultShape::Bound(5));
    let hold = by("Hold");
    assert!(hold.options.blocks);
    assert_eq!(hold.options.route, Route::Keyed { group: "fixture-repo".into(), key: "repo".into() });
    assert_eq!(hold.route_key.as_ref().map(|k| k.number), Some(1));
    assert!(by("Run").options.runs_to_completion);
    // One person's act of each effect, so a bridge's mapping has both cases (C, D).
    use anemnez_extension::options::Effect;
    assert_eq!((hold.options.effect, hold.options.person_act), (Effect::Write, true));
    assert_eq!((by("Run").options.effect, by("Run").options.person_act), (Effect::Act, true));
    assert!(!by("Wait").options.runs_to_completion, "Wait owns its budget by its field, not the flag");
    assert_eq!(
        by("Items").options.examples,
        [anemnez_extension::options::Example::new(r#"{"cursor": ""}"#, "The first page")]
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
    for bridges in [Variant::WrongMajor, Variant::CollidingNames, Variant::ReservedName] {
        assert!(names(bridges).is_empty(), "{bridges:?} is the bridge's to refuse");
    }
    assert_eq!((Variant::WrongMajor.contract(), Variant::WrongMajor.version()), ("fixture", "2.0.0"));
    assert_eq!(Variant::ReservedName.contract(), "string");
    let colliding = check_descriptor(Variant::CollidingNames.file_descriptor_set()).expect("admitted");
    let methods: Vec<_> = colliding.iter().map(|m| m.path.rsplit('/').next().unwrap_or_default()).collect();
    assert_eq!(methods, ["GetURL", "GetUrl"]);
}

/// CONTRACT.md one-mapping-flattens-a-descriptor and a-flattened-name-is-one-tool, on descriptors
/// protoc wrote: the good fixture's agent methods are its tools (never `Ping`, which has no anemnez
/// options), with events and the log last; `CollidingNames`, admitted by the contract's check, has no face.
#[test]
fn the_fixture_flattens_and_its_colliding_names_do_not() {
    use anemnez_extension::mcp::{FlattenRefusal, flatten};
    let fds = Variant::Good.file_descriptor_set();
    let good =
        flatten(&service_of(fds).expect("declared"), &check_descriptor(fds).expect("admitted")).expect("flattens");
    let names: Vec<&str> = good.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["items", "recent", "get", "hold", "run", "wait", "snapshot", "orient", "events", "log_read"]);
    let long: Vec<&str> = good.iter().filter(|t| t.long).map(|t| t.name.as_str()).collect();
    assert_eq!(long, ["hold", "run"], "blocks and runs_to_completion are long; Wait owns its budget");
    let fds = Variant::CollidingNames.file_descriptor_set();
    let colliding = check_descriptor(fds).expect("admitted");
    assert_eq!(
        flatten(&service_of(fds).expect("declared"), &colliding).expect_err("refused"),
        [FlattenRefusal::NameCollision(
            "get_url".into(),
            "/anemnez.fixture.collidingnames.v1.Fixture/GetURL".into(),
            "/anemnez.fixture.collidingnames.v1.Fixture/GetUrl".into(),
        )]
    );
}

fn with_token<T>(message: T, token: &str) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    request.metadata_mut().insert(anemnez_extension::principal::GRPC_METADATA_KEY, token.parse().expect("ascii"));
    request
}

/// Over the fixture's real socket: every method answers its shape, a carried token's digest comes
/// back, and Describe, Health and Events are the contract's.
#[tokio::test(flavor = "multi_thread")]
async fn the_good_fixture_answers_over_its_socket() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), manual.clock(), Variant::Good).await.expect("serves");
    let channel = running.channel().await.expect("connects");
    let mut fixture = pb::fixture_client::FixtureClient::new(channel.clone());

    // Items pages to the end.
    let (mut seen, mut cursor) = (Vec::new(), String::new());
    loop {
        let page = fixture.items(pb::ItemsRequest { cursor }).await.expect("a page").into_inner();
        assert!(page.items.len() <= 5);
        seen.extend(page.items.into_iter().map(|i| i.id));
        if page.next.is_empty() {
            break;
        }
        cursor = page.next;
    }
    assert_eq!(seen.len(), 12);

    let recent = fixture.recent(pb::RecentRequest {}).await.expect("recent").into_inner();
    assert_eq!(recent.items.len(), 5);
    let got = fixture.get(pb::GetRequest { id: "i03".into() }).await.expect("got").into_inner();
    assert_eq!(got.name, "item 3");
    let missing = fixture.get(pb::GetRequest { id: "nope".into() }).await.expect_err("missing");
    assert_eq!(missing.code(), tonic::Code::NotFound);

    // A token rides the call, and its digest comes back.
    let held = fixture
        .hold(with_token(pb::HoldRequest { repo: "agora".into(), why: "test".into() }, "token-abc"))
        .await
        .expect("held");
    let echoed = held.metadata().get(DIGEST_KEY).and_then(|v| v.to_str().ok()).map(str::to_owned);
    assert_eq!(echoed, Some(Token::digest("token-abc")));
    let no_key = fixture.hold(pb::HoldRequest::default()).await.expect_err("no key");
    assert_eq!((no_key.code(), no_key.message()), (tonic::Code::InvalidArgument, "no-key"));

    fixture.run(pb::RunRequest {}).await.expect("ran");
    fixture.ping(pb::PingRequest {}).await.expect("pong");

    // Wait owns its budget on the fixture's clock: it answers only once that clock has moved.
    let mut waiting = fixture.clone();
    let wait = tokio::spawn(async move { waiting.wait(pb::WaitRequest { timeout_ms: 30_000 }).await });
    assert!(manual.until_waiting(1, Duration::from_secs(30)).await, "the wait parks on the clock");
    assert!(!wait.is_finished());
    manual.advance(Duration::from_secs(30)).await;
    assert_eq!(wait.await.expect("joins").expect("waited").into_inner().waited_ms, 30_000);

    assert_eq!(running.contract, "fixture");
}

/// The contract's own services on the fixture: Describe serves the variant's descriptor, Health is
/// state with a cursor, and Events after that cursor carries the next hold.
#[tokio::test(flavor = "multi_thread")]
async fn the_fixture_serves_the_contracts_own_services() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), manual.clock(), Variant::Good).await.expect("serves");
    let channel = running.channel().await.expect("connects");
    let mut fixture = pb::fixture_client::FixtureClient::new(channel.clone());
    // The contract's own services.
    use anemnez_extension_serve::pb as contract;
    let described = contract::describe_client::DescribeClient::new(channel.clone())
        .get(contract::DescribeRequest {})
        .await
        .expect("described")
        .into_inner();
    assert_eq!(described.file_descriptor_set, Variant::Good.file_descriptor_set());
    let health = contract::health_client::HealthClient::new(channel.clone())
        .get(contract::HealthRequest {})
        .await
        .expect("health")
        .into_inner();
    assert_eq!(health.state, contract::State::Serving as i32);
    let mut events = contract::events_client::EventsClient::new(channel)
        .events(contract::EventsRequest { after: health.as_of, kinds: vec!["held".into()] })
        .await
        .expect("opens")
        .into_inner();
    // Subscribed after Health's as_of, so the next hold is the first event it sees.
    fixture.hold(pb::HoldRequest { repo: "agora".into(), why: "after as_of".into() }).await.expect("held again");
    let event = events.message().await.expect("reads").expect("the hold's event");
    assert_eq!((event.kind.as_str(), event.payload), ("held", b"agora".to_vec()));
}

/// Snapshot's reply tags two bytes fields as images, one singular and one repeated, named with two words so a
/// bridge proves it matches the proto's own field names (CONTRACT.md a-bytes-field-may-say-it-is-an-image).
#[test]
fn snapshot_tags_its_image_fields() {
    use anemnez_extension::media::Media;
    let methods = check_descriptor(Variant::Good.file_descriptor_set()).expect("admitted");
    let snapshot = methods.iter().find(|m| m.path.ends_with("/Snapshot")).expect("Snapshot offered");
    assert_eq!(snapshot.output_media, [("page_png".to_owned(), Media::Png), ("frame_pngs".to_owned(), Media::Png)]);
    let untagged = methods.iter().filter(|m| !m.path.ends_with("/Snapshot"));
    assert!(untagged.into_iter().all(|m| m.output_media.is_empty()), "no other reply carries an image");
    assert!(crate::PNG_1X1.starts_with(b"\x89PNG\r\n\x1a\n") && crate::PNG_1X1.ends_with(b"IEND\xAE\x42\x60\x82"));
}

/// A host's late-orient proof needs a native orient that can be held: while held, Orient answers nothing until it is
/// let go (or its call is dropped), and then answers `ORIENT_NOW`. No timer: the fixture says when it holds one.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_orient_answers_only_once_let_go() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), ManualClock::new().clock(), Variant::Good).await.expect("serves");
    let mut fixture = pb::fixture_client::FixtureClient::new(running.channel().await.expect("connects"));
    running.hold_orient(true);
    let asked = tokio::spawn(async move { fixture.orient(pb::OrientRequest {}).await });
    running.orient_held().await;
    assert!(!asked.is_finished(), "held: no answer yet");
    running.hold_orient(false);
    let now = asked.await.expect("ran").expect("answered").into_inner().now;
    assert_eq!(now, crate::ORIENT_NOW);
}

/// M m/41794 (rule 4): a call held open across stop() doesn't hold stop for ever. The graceful end waits on every
/// in-flight call, so stop cuts it at STOP_WITHIN on the fixture's clock, aborts, and returns false. With no call held,
/// stop ends gracefully and returns true. A real-time guard makes a hang red rather than a hung row.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_held_across_stop_is_cut_at_its_bound() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), manual.clock(), Variant::Good).await.expect("serves");
    let mut fixture = pb::fixture_client::FixtureClient::new(running.channel().await.expect("connects"));
    running.hold_orient(true);
    let held = tokio::spawn(async move { fixture.orient(pb::OrientRequest {}).await });
    running.orient_held().await;
    let stopping = tokio::spawn(running.stop());
    assert!(manual.until_waiting(1, Duration::from_secs(5)).await, "stop's bound is armed");
    manual.advance(crate::STOP_WITHIN).await;
    let graceful = anemnez_clock::Clock::system()
        .timeout(Duration::from_secs(20), stopping)
        .await
        .expect("stop returns at its bound, never hangs")
        .expect("ran");
    assert!(!graceful, "a held call is cut at the bound, not waited on");
    // D m/41885: false means cut. The held call ends with an error, never left answering on a connection the abort
    // didn't reach.
    let ended = anemnez_clock::Clock::system()
        .timeout(Duration::from_secs(20), held)
        .await
        .expect("the held call ends once stop cuts it")
        .expect("ran");
    assert!(ended.is_err(), "the held call was cut, not answered: {ended:?}");

    let dir = tempfile::tempdir().expect("a temp dir");
    let idle = serve(dir.path(), ManualClock::new().clock(), Variant::Good).await.expect("serves");
    assert!(idle.stop().await, "with no call held, stop ends gracefully");
}

/// D m/42087: a connection's duplicate goes when its served stream ends, so a connection the server closes normally
/// isn't held open by the fixture, and the fixture holds none it no longer serves. Red when the duplicate is kept for
/// the fixture's whole life.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_connection_leaves_no_duplicate_behind() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), anemnez_clock::Clock::system(), Variant::Good).await.expect("serves");
    let mut fixture = pb::fixture_client::FixtureClient::new(running.channel().await.expect("connects"));
    fixture.orient(pb::OrientRequest {}).await.expect("answered");
    assert_eq!(running.open.count(), 1, "the served connection is held");
    drop(fixture);
    let open = running.open.clone();
    let clock = anemnez_clock::Clock::system();
    let gone = clock.until("the closed connection's duplicate to go", || {
        let n = open.count();
        async move { (n == 0).then_some(()) }
    });
    // A real-time guard, so a duplicate kept for ever is red in seconds, never a wait to the house backstop.
    clock
        .timeout(Duration::from_secs(5), gone)
        .await
        .expect("the closed connection's duplicate goes, never held for the fixture's life");
    assert!(running.stop().await, "and stop is graceful with nothing held");
}

/// B m/33548: the bounded twin names what didn't arrive when no Orient call is held within its bound, as an event on the
/// clock handed in (no sleep), and answers once one is.
#[tokio::test(flavor = "multi_thread")]
async fn orient_held_within_names_a_call_that_never_came() {
    let manual = ManualClock::new();
    let dir = tempfile::tempdir().expect("a temp dir");
    let running = serve(dir.path(), manual.clock(), Variant::Good).await.expect("serves");
    running.hold_orient(true);
    let clock = manual.clock();
    let (waited, ()) = tokio::join!(running.orient_held_within(&clock, Duration::from_secs(5)), async {
        assert!(manual.until_waiting(1, Duration::from_secs(5)).await, "the bound is armed");
        manual.advance(Duration::from_secs(5)).await;
    });
    assert!(waited.expect_err("bounded").contains("no Orient call reached the fixture within 5s"));
}

/// The fixture as a gate drives it for `#the-static-half-is-byte-stable`: up serves `variants[n]` for the nth time up
/// (the last one from then on), change publishes an event and holds then lets go its orient, down stops it.
struct Restarts {
    dir: tempfile::TempDir,
    clock: anemnez_clock::Clock,
    variants: Vec<Variant>,
    ups: usize,
    running: Option<crate::Running>,
}

impl Restarts {
    fn of(clock: anemnez_clock::Clock, variants: Vec<Variant>) -> Self {
        Restarts { dir: tempfile::tempdir().expect("a temp dir"), clock, variants, ups: 0, running: None }
    }
}

#[tonic::async_trait]
impl anemnez_extension_serve::stable::Restartable for Restarts {
    async fn up(&mut self) -> Result<tonic::transport::Channel, String> {
        let variant = self.variants[self.ups.min(self.variants.len() - 1)];
        self.ups += 1;
        let running = serve(self.dir.path(), self.clock.clone(), variant).await.map_err(|e| e.to_string())?;
        let channel = running.channel().await.map_err(|e| e.to_string())?;
        self.running = Some(running);
        Ok(channel)
    }

    async fn change(&mut self) -> Result<(), String> {
        let running = self.running.as_ref().ok_or("not up")?;
        running.journal.publish("moved", b"a live change".to_vec());
        running.hold_orient(true);
        running.hold_orient(false);
        Ok(())
    }

    async fn down(&mut self) {
        if let Some(running) = self.running.take() {
            let socket = running.socket.clone();
            // Ended and awaited, so the driver's proof that it's down holds (N m/41684): dropping it alone left its
            // open connection answering.
            let _ = running.stop().await;
            let _ = std::fs::remove_file(socket);
        }
    }
}

/// ORIENTATION.md `#the-static-half-is-byte-stable`, the row each product's gate runs (K), here over the fixture: its
/// guidance, read as Agora reads it (Describe, then mcp::instructions), is the same bytes across a change of state
/// and across going down and back.
#[tokio::test(flavor = "multi_thread")]
async fn the_fixtures_static_half_is_byte_stable() {
    let clock = ManualClock::new().clock();
    let mut fixture = Restarts::of(clock.clone(), vec![Variant::Good]);
    anemnez_extension_serve::stable::the_static_half_is_byte_stable(&mut fixture, &clock).await.expect("byte-stable");
    assert_eq!(fixture.ups, 2, "read up, after a change, and after a restart");
}

/// The same check is red for a product whose static half isn't the same after a restart (here, a build whose
/// guidance is gone), and says which read differed and both texts.
#[tokio::test(flavor = "multi_thread")]
async fn a_static_half_that_changes_across_a_restart_is_red() {
    let clock = ManualClock::new().clock();
    let mut fixture = Restarts::of(clock.clone(), vec![Variant::Good, Variant::WrongMajor]);
    let red =
        anemnez_extension_serve::stable::the_static_half_is_byte_stable(&mut fixture, &clock).await.expect_err("red");
    assert!(red.contains("the static half's instructions changed after it went down and came back"), "{red}");
    assert!(red.contains("Recent is the newest five") && red.contains("then:  \"\""), "{red}");
    assert!(fixture.running.is_none(), "down, whatever it answered");
}
