//! The API plane's per-call rules from the peer its accept judged (CONTRACT.md §8, §8.1), applied once, below
//! every service, by [`ApiPlaneLayer`]. They read the [`Role`] that `product::planes` decided at accept and
//! never judge identity again:
//!
//! - a call on a connection that wasn't judged at accept is refused, `connection not judged at accept`, never
//!   served as an ordinary caller (D m/13648);
//! - fabricd's peer gets `Describe` only, and any other call is refused `fabricd-reads-describe-only`;
//! - a call that carries a caller's stamp from any peer but Agora is refused `stamp-not-from-agora`;
//! - a carried call (Aegis's peer) reaches a method only when its descriptor says `read`, or one of the contract's
//!   own Describe or Health, else it is refused `carried-write-unbound` ([`carried_admits`]; K m/23155). The log's
//!   Events are refused to a carried call whole, since Call carries an opaque request (E m/23165);
//! - a carried call to a write its product lists for claimed lanes ([`ClaimedWrites`], D m/28960) is admitted with
//!   its stamp's lane as a claim, [`Claimed`] `{ lane, via }` on the request, when aegisd said the node it came from
//!   and the stamp names one of the product's lanes, by its own predicate ([`carried_claim`]); else `carried-no-preface` or
//!   `carried-claim-not-a-lane`.
//!
//! - a carried call to a method whose descriptor says `callers = PRODUCT` ([`ProductCallers`], D m/32662) is admitted
//!   only when aegisd's preface carries the far caller's code identity, its team the house's (anemnez-trust's anchor),
//!   and its identifier on the method's allow-list, which the product names (D m/32983: the house's CLIs are team-signed
//!   too, so the team alone never admits). [`ProductCaller`] `{ node, identity }` rides the request for the handler's
//!   own further narrowing (for a replica stream, the assignment). An absent identity is `product-caller-unproven`, one
//!   meaning whatever the cause; another team is `product-caller-untrusted`; an identifier not listed is
//!   `product-caller-not-allowed`; and a method its product named no callers for is `product-callers-unnamed`, failing
//!   closed ([`product_admits`]). A carried agent write still needs its bound token: a code identity never opens a
//!   method that isn't `callers = PRODUCT`.
//!
//! Which principal a call then runs as, and whether a grant covers it, is the product's own decision
//! (`one-decision-per-call-at-the-product`); these rules only say who may reach it at all.

use std::task::{Context, Poll};

use anemnez_extension::mcp::STAMP_METADATA_KEY;
use std::collections::HashSet;
use std::sync::Arc;

use anemnez_extension::mcp::Stamp;
use anemnez_extension::options::Effect;
use anemnez_extension::peer::{
    CARRIED_CLAIM_NOT_A_LANE, CARRIED_NO_PREFACE, CARRIED_WRITE_UNBOUND, CLAIMED_METHOD_NOT_A_READ_OR_WRITE,
    CLAIMED_WRITE_NOT_A_WRITE, FABRICD_READS_DESCRIBE_ONLY, NOT_JUDGED, PRODUCT_CALLER_NOT_ALLOWED,
    PRODUCT_CALLER_UNPROVEN, PRODUCT_CALLER_UNTRUSTED, PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD, PRODUCT_CALLERS_UNNAMED,
    Role, STAMP_NOT_FROM_AGORA,
};
use anemnez_extension::teaching::{RetryClass, Teaching, Whose, status_details};
use tonic::body::BoxBody;

use crate::accepted::Judged;

/// The path prefix of the contract's `Describe` service, the one thing fabricd reads on the API plane.
pub const DESCRIBE_PREFIX: &str = "/anemnez.extension.v1.Describe/";

/// Whether the API plane serves a call to `path` from the connection `judged` (None: not judged at accept),
/// carrying a caller's stamp or not. `Err` is the refusal's name. Pure, so every rule has a row of its own.
pub fn api_admits(judged: Option<&Judged>, path: &str, carries_stamp: bool) -> Result<(), &'static str> {
    let Some(judged) = judged else {
        return Err(NOT_JUDGED);
    };
    match judged.role {
        Role::Fabricd if !path.starts_with(DESCRIBE_PREFIX) => Err(FABRICD_READS_DESCRIBE_ONLY),
        Role::Agora => Ok(()),
        // Every other role, a future one included, is not Agora: its stamp is never believed.
        _ if carries_stamp => Err(STAMP_NOT_FROM_AGORA),
        _ => Ok(()),
    }
}

/// The contract's own faces a carried call may read: Describe and Health. Never the log's Events: Call carries
/// hydramq-events' wire request opaquely, so whether one is a read isn't the plane's to know, and a commit or an ack
/// through it would be a write past the descriptor's effects (E m/23165). A carried read of the log waits on a
/// rule that decodes it, or the caller's binding.
const FACE_READS: [&str; 2] = ["/anemnez.extension.v1.Describe/", "/anemnez.extension.v1.Health/"];

/// The product's methods a carried call may reach: those its descriptor says are `read`, read once when the plane
/// is built, never per call. A descriptor `check_descriptor` doesn't admit reads as none, so every carried call to a
/// product method is refused, the safe direction.
#[derive(Clone, Debug, Default)]
pub struct Reads(pub(crate) Arc<HashSet<String>>);

impl Reads {
    /// The read methods of the encoded `FileDescriptorSet`.
    #[must_use]
    pub fn of(file_descriptor_set: &[u8]) -> Reads {
        let reads = anemnez_extension::descriptor::check_descriptor(file_descriptor_set)
            .map(|methods| methods.into_iter().filter(|m| m.options.effect == Effect::Read).map(|m| m.path).collect())
            .unwrap_or_default();
        Reads(Arc::new(reads))
    }

    pub(crate) fn reads(&self, path: &str) -> bool {
        self.0.contains(path) || FACE_READS.iter().any(|face| path.starts_with(face))
    }
}

/// The product's methods a product calls (`callers = PRODUCT` in its descriptor), read once when the plane is built, each
/// with the signing identifiers its product allows to call it (D m/32983). A carried call reaches one past the read rule
/// only from a proven product on its list ([`product_admits`]). A method with no list admits nobody, and a descriptor
/// `check_descriptor` doesn't admit lists no method, the safe direction both ways.
#[derive(Clone, Debug, Default)]
pub struct ProductCallers {
    methods: Arc<HashSet<String>>,
    allowed: Arc<std::collections::HashMap<String, Vec<String>>>,
}

impl ProductCallers {
    /// The `callers = PRODUCT` methods of the encoded `FileDescriptorSet`, with `allow`, each `(method, identifiers)`.
    ///
    /// # Errors
    /// [`PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD`] and the method, for an entry the descriptor doesn't say is
    /// `callers = PRODUCT`, or that names no identifier.
    pub fn of(file_descriptor_set: &[u8], allow: &[(&str, &[&str])]) -> Result<ProductCallers, (&'static str, String)> {
        let methods: HashSet<String> = anemnez_extension::descriptor::check_descriptor(file_descriptor_set)
            .map(|methods| {
                methods
                    .into_iter()
                    .filter(|m| m.options.callers == anemnez_extension::options::Callers::Product)
                    .map(|m| m.path)
                    .collect()
            })
            .unwrap_or_default();
        let mut allowed = std::collections::HashMap::new();
        for (method, identifiers) in allow {
            if !methods.contains(*method) || identifiers.is_empty() {
                return Err((PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD, (*method).to_owned()));
            }
            allowed.insert((*method).to_owned(), identifiers.iter().map(|i| (*i).to_owned()).collect());
        }
        Ok(ProductCallers { methods: Arc::new(methods), allowed: Arc::new(allowed) })
    }

    /// Whether `path` is one a product calls.
    #[must_use]
    pub fn lists(&self, path: &str) -> bool {
        self.methods.contains(path)
    }

    /// The identifiers allowed to call `path`, if its product named any.
    #[must_use]
    pub fn allowed(&self, path: &str) -> Option<&[String]> {
        self.allowed.get(path).map(Vec::as_slice)
    }
}

/// A carried product caller, proven and allowed: the node aegisd said the call came from, and the code identity it
/// judged of the far process. On the request, for the handler's further narrowing (D m/32662).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductCaller {
    pub node: String,
    pub identity: anemnez_extension::carried::CodeIdentity,
}

/// Whether the carried call `judged` to a method allowing `allowed` (None: its product named none) is from a proven,
/// allowed product: aegisd's preface carries the far caller's code identity, its team the house's (anemnez-trust's
/// anchor, never "the far aegisd said so"), and its identifier on the list. Pure.
///
/// # Errors
/// [`PRODUCT_CALLERS_UNNAMED`] when the method has no list; [`PRODUCT_CALLER_UNPROVEN`] with no preface or no code
/// identity in it, one meaning whatever the cause; [`PRODUCT_CALLER_UNTRUSTED`] for another team;
/// [`PRODUCT_CALLER_NOT_ALLOWED`] for an identifier not on the list.
pub fn product_admits(judged: &Judged, allowed: Option<&[String]>) -> Result<ProductCaller, &'static str> {
    let Some(allowed) = allowed else { return Err(PRODUCT_CALLERS_UNNAMED) };
    let Some(preface) = &judged.carried else { return Err(PRODUCT_CALLER_UNPROVEN) };
    let Some(identity) = &preface.code else { return Err(PRODUCT_CALLER_UNPROVEN) };
    if identity.team != anemnez_trust::ANEMNEZ_TEAM {
        return Err(PRODUCT_CALLER_UNTRUSTED);
    }
    if !allowed.contains(&identity.identifier) {
        return Err(PRODUCT_CALLER_NOT_ALLOWED);
    }
    Ok(ProductCaller { node: preface.node.clone(), identity: identity.clone() })
}

/// Whether a call to `path` on a connection judged `role` may go on, beyond [`api_admits`]: a carried call only to a
/// read. Pure, so the rule has its row.
///
/// # Errors
/// [`CARRIED_WRITE_UNBOUND`] for a carried call to anything but a read.
pub fn carried_admits(role: Role, path: &str, reads: &Reads) -> Result<(), &'static str> {
    match role {
        Role::Carried if !reads.reads(path) => Err(CARRIED_WRITE_UNBOUND),
        _ => Ok(()),
    }
}

/// What a product says a lane is: its own predicate, since the shared plane can't know a product's lanes (D m/29077).
type IsLane = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The methods a product lists for claimed lanes, and what it says a lane is (D m/29077): opted in per method, never per
/// product, each a `read` or a `write` in the product's own descriptor (D m/28960 (1), m/29220).
/// - A listed **write** is admitted from a carried call only with a claim that holds, else refused.
/// - A listed **read** is admitted whatever its claim, as every carried read is. A claim that holds rides it for
///   attribution, metering and fairness only, and one that doesn't is served unattributed, never refused.
///
/// Default none: every carried write is refused `carried-write-unbound` and every carried read unattributed. Which of
/// its methods to list is the product's own call: the check refuses an `act`, but a product whose descriptor says a
/// host-touching verb is a `write` keeps it out by its own list.
#[derive(Clone)]
pub struct ClaimedMethods {
    writes: Arc<HashSet<String>>,
    reads: Arc<HashSet<String>>,
    is_lane: IsLane,
}

/// [`ClaimedMethods`] by the name its first adopters took (Ergon's): one type, never a second copy (rule 2).
pub type ClaimedWrites = ClaimedMethods;

impl Default for ClaimedMethods {
    fn default() -> ClaimedMethods {
        ClaimedMethods { writes: Arc::default(), reads: Arc::default(), is_lane: Arc::new(|_| false) }
    }
}

impl std::fmt::Debug for ClaimedMethods {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimedMethods")
            .field("writes", &self.writes)
            .field("reads", &self.reads)
            .finish_non_exhaustive()
    }
}

impl ClaimedMethods {
    /// `methods` (gRPC paths, `/pkg.Service/Method`), each checked against the encoded `FileDescriptorSet` the plane
    /// serves and sorted by its effect there, and `is_lane`, the product's own word on whether a claimed name is one of
    /// its lanes (Ergon's: an owner that holds a book). No product opts in without saying what a lane is.
    ///
    /// # Errors
    /// [`CLAIMED_METHOD_NOT_A_READ_OR_WRITE`] and the method, for one the descriptor says is an `act`, or a path it
    /// doesn't have. A descriptor the check doesn't admit has no methods, so any list is refused.
    pub fn of(
        file_descriptor_set: &[u8],
        methods: &[&str],
        is_lane: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Result<ClaimedMethods, (&'static str, String)> {
        let effects: std::collections::HashMap<String, Effect> =
            anemnez_extension::descriptor::check_descriptor(file_descriptor_set)
                .map(|all| all.into_iter().map(|m| (m.path, m.options.effect)).collect())
                .unwrap_or_default();
        let (mut writes, mut reads) = (HashSet::new(), HashSet::new());
        for method in methods {
            match effects.get(*method) {
                Some(Effect::Write) => writes.insert((*method).to_owned()),
                Some(Effect::Read) => reads.insert((*method).to_owned()),
                _ => return Err((CLAIMED_METHOD_NOT_A_READ_OR_WRITE, (*method).to_owned())),
            };
        }
        Ok(ClaimedMethods { writes: Arc::new(writes), reads: Arc::new(reads), is_lane: Arc::new(is_lane) })
    }

    pub(crate) fn lists_write(&self, path: &str) -> bool {
        self.writes.contains(path)
    }

    pub(crate) fn lists_read(&self, path: &str) -> bool {
        self.reads.contains(path)
    }
}

/// A carried write admitted for a claimed lane (D m/28960): the lane the far caller's stamp claims, and the node aegisd
/// said the call came from. A claim, recorded as one (`lane:<l> claimed via <node>`), never a verified principal: the
/// carrier is verified, the claim is not. The plane puts it on the request's extensions; read it with
/// `request.extensions().get::<Claimed>()`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Claimed {
    pub lane: String,
    pub via: String,
}

/// The longest name a claimed lane may be.
const LANE_MAX: usize = 64;

/// A lane's shape, beneath the product's own word on it: a plain name (ASCII alphanumeric and `-`, at most
/// [`LANE_MAX`]), never `:`, `/`, `@` or a space, since the lane is written into a recorded principal (D m/29077).
fn lane_shaped(lane: &str) -> bool {
    !lane.is_empty() && lane.len() <= LANE_MAX && lane.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The claim of a carried call to a listed write, from what its accept judged and the stamp bytes it carries, held to
/// the product's own word on what a lane is. Pure.
///
/// # Errors
/// [`CARRIED_NO_PREFACE`] when aegisd said no node; [`CARRIED_CLAIM_NOT_A_LANE`] for no stamp, a stamp with no lane, a
/// lane not shaped as a plain name, or one the product says isn't its lane.
pub fn carried_claim(judged: &Judged, stamp: Option<&[u8]>, claimed: &ClaimedMethods) -> Result<Claimed, &'static str> {
    let via = judged.carried.as_ref().map(|preface| preface.node.clone()).ok_or(CARRIED_NO_PREFACE)?;
    let stamp = Stamp::from_metadata(stamp.ok_or(CARRIED_CLAIM_NOT_A_LANE)?);
    let lane = stamp.claimed_lane().ok_or(CARRIED_CLAIM_NOT_A_LANE)?;
    if !lane_shaped(lane) || !(claimed.is_lane)(lane) {
        return Err(CARRIED_CLAIM_NOT_A_LANE);
    }
    Ok(Claimed { lane: lane.to_owned(), via })
}

/// Every API-plane rule for one call, decided once: a carried call to a listed write by [`carried_claim`], answering
/// its claim; a carried call to a read admitted as a read, its stamp unused; every other call by [`api_admits`] then
/// [`carried_admits`]. Pure, so each rule has its row.
///
/// # Errors
/// The refusal's name.
pub fn plane_admits(
    judged: Option<&Judged>,
    path: &str,
    stamp: Option<&[u8]>,
    reads: &Reads,
    claimed: &ClaimedMethods,
) -> Result<Option<Claimed>, &'static str> {
    match judged {
        Some(j) if j.role == Role::Carried && claimed.lists_write(path) => carried_claim(j, stamp, claimed).map(Some),
        // A carried read is admitted as a read whatever it carries: Agora sends its stamp on every far call, so reads
        // stay location agnostic (O m/29572). Only a read its product lists has the stamp read at all, and then a
        // claim that holds is attribution only, and one that doesn't is no claim, never a refusal (D m/29220).
        Some(j) if j.role == Role::Carried && reads.reads(path) => {
            Ok(if claimed.lists_read(path) { carried_claim(j, stamp, claimed).ok() } else { None })
        }
        // A carried write its product didn't list is carried-write-unbound, stamp or none: the method isn't a claimed
        // write here, which is what the lane must hear, never stamp-not-from-agora, which tells it its caller is wrong
        // (D m/30197, rule 15).
        Some(j) if j.role == Role::Carried => Err(CARRIED_WRITE_UNBOUND),
        _ => api_admits(judged, path, stamp.is_some())
            .and_then(|()| judged.map_or(Ok(()), |j| carried_admits(j.role, path, reads)))
            .map(|()| None),
    }
}

/// A plane refusal as the caller reads it, in the house form `<name>: <why>` (M m/25549, G m/25537): the name first,
/// so a reader matches it by its name (anemnez-service `reach::named`), and why, so a person reads more than the name.
#[must_use]
pub fn said(name: &'static str) -> String {
    let why = match name {
        // Already a sentence a person reads, and unchanged on the wire.
        NOT_JUDGED => return name.to_owned(),
        FABRICD_READS_DESCRIBE_ONLY => "fabricd reads Describe on the API plane and nothing else",
        STAMP_NOT_FROM_AGORA => "a caller's stamp is believed only from Agora's judged peer",
        CARRIED_WRITE_UNBOUND => {
            "a call carried from another host reaches reads only, until its caller's principal is bound end to end"
        }
        CARRIED_NO_PREFACE => {
            "a carried write is admitted for a claimed lane only when aegisd says the node it came from"
        }
        CARRIED_CLAIM_NOT_A_LANE => "a carried write's stamp must claim one of the product's lanes, by a plain name",
        CLAIMED_WRITE_NOT_A_WRITE => "only a method the descriptor says is a write may be listed for claimed lanes",
        CLAIMED_METHOD_NOT_A_READ_OR_WRITE => {
            "only a method the descriptor says is a read or a write may be listed for claimed lanes, never an act"
        }
        PRODUCT_CALLER_UNPROVEN => {
            "a carried call to a method products call is admitted only when aegisd says who signed the far caller"
        }
        PRODUCT_CALLER_UNTRUSTED => "a carried product caller must be signed by the house's team",
        PRODUCT_CALLERS_UNNAMED => "a method products call admits a carried caller only once its product names who may",
        PRODUCT_CALLER_NOT_ALLOWED => "a carried product caller must be one the method's product names as its caller",
        PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD => {
            "only a method the descriptor says products call may list its callers, and it must name at least one"
        }
        _ => "refused by the API plane",
    };
    format!("{name}: {why}")
}

/// A plane refusal of a call to `path`, as [`said`] says it, and for `carried-write-unbound` naming the method, so the
/// lane learns which of its calls isn't a claimed write here (D m/30197, rule 15).
#[must_use]
pub fn said_on(name: &'static str, path: &str) -> String {
    match name {
        CARRIED_WRITE_UNBOUND => {
            let method = path.rsplit('/').next().filter(|m| !m.is_empty()).unwrap_or(path);
            format!("{} ({method} isn't a claimed write here)", said(name))
        }
        _ => said(name),
    }
}

/// Every name the API plane refuses with, at a call or when the plane is built: the one list [`said`] and [`taught`]
/// answer for, held by a row that reads this file's own refusals, so a new name can't go untaught (D m/30986).
pub const PLANE_REFUSALS: [&str; 13] = [
    NOT_JUDGED,
    FABRICD_READS_DESCRIBE_ONLY,
    STAMP_NOT_FROM_AGORA,
    CARRIED_WRITE_UNBOUND,
    CARRIED_NO_PREFACE,
    CARRIED_CLAIM_NOT_A_LANE,
    CLAIMED_WRITE_NOT_A_WRITE,
    CLAIMED_METHOD_NOT_A_READ_OR_WRITE,
    PRODUCT_CALLER_UNPROVEN,
    PRODUCT_CALLER_UNTRUSTED,
    PRODUCT_CALLERS_UNNAMED,
    PRODUCT_CALLER_NOT_ALLOWED,
    PRODUCT_CALLERS_NOT_A_PRODUCT_METHOD,
];

/// A plane refusal of a call to `path` as a teaching (house rule 15, AX3): its name as the code, what `said_on` says
/// as what was expected, a retry class, and whose it is to fix. Every plane refusal is refused before any work, and the
/// same call is refused again, so each is `Blanket`. What differs is whose it is: the caller's call to change, or the
/// operator's wiring of this host.
#[must_use]
pub fn taught(name: &'static str, path: &str) -> Teaching {
    let (code, whose) = match name {
        // A server whose accept judged nothing, or a carried call no aegisd vouched for: this host's wiring.
        NOT_JUDGED => ("not-judged", Whose::Operator),
        CARRIED_NO_PREFACE => (name, Whose::Operator),
        // Every other is the call itself: through Agora, a read, a listed write, a plain lane name.
        _ => (name, Whose::Caller),
    };
    let said = said_on(name, path);
    let expected = said.split_once(": ").map_or(said.as_str(), |(_, why)| why).to_owned();
    Teaching::refused(code, expected, RetryClass::Blanket, whose)
}

/// A plane refusal of a call to `path` as gRPC answers it: PERMISSION_DENIED, the house-form message `said_on` says
/// (what every reader matches by name, unchanged), and its [`taught`] teaching as the Status's one detail, so a reader
/// that knows the outcome types reads the retry class and whose (anemnez-extension `teaching::teaching_in`).
#[must_use]
pub fn refused(name: &'static str, path: &str) -> tonic::Status {
    let said = said_on(name, path);
    let details = status_details(tonic::Code::PermissionDenied as i32, &said, &taught(name, path));
    tonic::Status::with_details(tonic::Code::PermissionDenied, said, bytes::Bytes::from(details))
}

/// The API plane's rules as a layer on the product's server: each call is admitted by [`api_admits`] and
/// [`carried_admits`] or answered PERMISSION_DENIED with the refusal's name and why ([`said`]), before any service
/// sees it.
#[derive(Clone, Debug, Default)]
pub struct ApiPlaneLayer {
    reads: Reads,
    claimed: ClaimedWrites,
    products: ProductCallers,
}

impl ApiPlaneLayer {
    /// The layer for a product whose read methods are `reads`, admitting no claimed carried writes.
    #[must_use]
    pub fn new(reads: Reads) -> ApiPlaneLayer {
        ApiPlaneLayer { reads, claimed: ClaimedWrites::default(), products: ProductCallers::default() }
    }

    /// The same, admitting carried calls with a claimed lane to `claimed` (D m/28960).
    #[must_use]
    pub fn claiming(self, claimed: ClaimedWrites) -> ApiPlaneLayer {
        ApiPlaneLayer { claimed, ..self }
    }

    /// The same, admitting carried calls from proven, allowed products to `products` (D m/32662, m/32983).
    #[must_use]
    pub fn products(self, products: ProductCallers) -> ApiPlaneLayer {
        ApiPlaneLayer { products, ..self }
    }
}

impl<S> tower_layer::Layer<S> for ApiPlaneLayer {
    type Service = ApiPlaneService<S>;

    fn layer(&self, inner: S) -> ApiPlaneService<S> {
        ApiPlaneService {
            inner,
            reads: self.reads.clone(),
            claimed: self.claimed.clone(),
            products: self.products.clone(),
        }
    }
}

/// What an admitted call carries onto its request.
enum Admitted {
    Claimed(Claimed),
    Product(ProductCaller),
}

/// [`ApiPlaneLayer`]'s service.
#[derive(Clone, Debug)]
pub struct ApiPlaneService<S> {
    inner: S,
    reads: Reads,
    claimed: ClaimedWrites,
    products: ProductCallers,
}

impl<S> tower_service::Service<http::Request<BoxBody>> for ApiPlaneService<S>
where
    S: tower_service::Service<http::Request<BoxBody>, Response = http::Response<BoxBody>>,
    S::Future: Send + 'static,
{
    type Response = http::Response<BoxBody>;
    type Error = S::Error;
    type Future =
        futures_util::future::Either<std::future::Ready<Result<http::Response<BoxBody>, S::Error>>, S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    /// Decided before the inner service is touched, so the inner that was polled ready serves an admitted
    /// call itself, with no clone and no box.
    fn call(&mut self, mut request: http::Request<BoxBody>) -> Self::Future {
        use futures_util::future::Either;
        let admitted = {
            let stamp = request.headers().get(STAMP_METADATA_KEY).map(|v| stamp_bytes(v.as_bytes()));
            let judged = request.extensions().get::<Judged>();
            let path = request.uri().path();
            match judged {
                // A method products call, carried: admitted only from a proven product, its caller on the request.
                Some(j) if j.role == Role::Carried && self.products.lists(path) => {
                    match product_admits(j, self.products.allowed(path)) {
                        Ok(caller) => Ok(Some(Admitted::Product(caller))),
                        Err(name) => Err(name),
                    }
                }
                _ => plane_admits(judged, path, stamp.as_deref(), &self.reads, &self.claimed)
                    .map(|claimed| claimed.map(Admitted::Claimed)),
            }
        };
        match admitted {
            Err(name) => {
                let refused = answer_refusal(
                    request.headers(),
                    refused(name, request.uri().path()),
                    http::StatusCode::FORBIDDEN,
                    None,
                );
                Either::Left(std::future::ready(Ok(refused)))
            }
            Ok(admitted) => {
                match admitted {
                    Some(Admitted::Claimed(claimed)) => {
                        request.extensions_mut().insert(claimed);
                    }
                    Some(Admitted::Product(caller)) => {
                        request.extensions_mut().insert(caller);
                    }
                    None => {}
                }
                Either::Right(self.inner.call(request))
            }
        }
    }
}

/// Whether a request is a gRPC call, by its content type (`application/grpc`, `application/grpc+proto`, ...): what
/// decides whether a refusal is said as gRPC trailers or as an HTTP status.
#[must_use]
pub fn is_grpc(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/grpc"))
}

/// A refusal as its caller reads it (D m/30486 (3)): to a gRPC call, `status` as trailers, as before; to any other
/// request, the HTTP status `http`, its body `status`'s message (the house form, `<name>: <why>`) as plain text, and
/// `Retry-After` in whole seconds when it is not-now. Never an HTTP 200 carrying a grpc-status, which a REST client
/// reads as an empty success. One shape for every layer that refuses on the API plane: the plane's 403 here, the drain
/// layer's 503 (anemnez-service, M m/30486).
#[must_use]
pub fn answer_refusal(
    headers: &http::HeaderMap,
    status: tonic::Status,
    http: http::StatusCode,
    retry_after: Option<std::time::Duration>,
) -> http::Response<BoxBody> {
    if is_grpc(headers) {
        return status.into_http();
    }
    let body = http_body_util::Full::new(bytes::Bytes::from(status.message().to_owned()));
    let mut response = http::Response::new(tonic::body::boxed(body));
    *response.status_mut() = http;
    let text = http::HeaderValue::from_static("text/plain; charset=utf-8");
    response.headers_mut().insert(http::header::CONTENT_TYPE, text);
    if let Some(after) = retry_after {
        let seconds = after.as_secs() + u64::from(after.subsec_nanos() > 0);
        response.headers_mut().insert(http::header::RETRY_AFTER, http::HeaderValue::from(seconds));
    }
    response
}

/// A binary metadata value's bytes: gRPC carries a `-bin` value base64, padded or not. A value that doesn't decode
/// reads as no bytes, which parses as no stamp, so it claims no lane.
pub(crate) fn stamp_bytes(header: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const ANY_PADDING: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    ANY_PADDING.decode(header).unwrap_or_default()
}
