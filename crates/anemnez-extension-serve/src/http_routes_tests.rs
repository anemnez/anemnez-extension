//! A product's own HTTP routes on the API plane (`ProductRoutes::http`; D m/30486, F m/30196): served behind the same
//! layers, opened as HTTP/1 only where a product declares them and only for an uncarried peer, refused as real HTTP
//! statuses, each request carrying its connection's verdict.

use anemnez_clock::ManualClock;
use anemnez_extension::peer::{FABRICD_READS_DESCRIBE_ONLY, Role, SERVICE_NAMED_TWICE};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::serve::{ProductRoutes, Stop, serve};
use crate::{Accepted, Judged};

/// A product's HTTP routes: `/api/who` answers the judged role it was handed, `/api/count` the bytes it was sent.
fn router() -> axum::Router {
    use axum::routing::{get, put};
    axum::Router::new()
        .route(
            "/api/who",
            get(|request: axum::extract::Request| async move {
                format!("{:?}", request.extensions().get::<Judged>().map(|j| j.role))
            }),
        )
        .route(
            "/api/count",
            put(|request: axum::extract::Request| async move {
                use http_body_util::BodyExt as _;
                let mut body = request.into_body();
                let mut n = 0usize;
                while let Some(frame) = body.frame().await {
                    n += frame.ok().and_then(|f| f.into_data().ok()).map_or(0, |d| d.len());
                }
                n.to_string()
            }),
        )
}

/// A plane serving `routes` to connections judged `role`; answers the dir (keep it) and the socket.
fn plane(role: Role, routes: ProductRoutes) -> (tempfile::TempDir, std::path::PathBuf) {
    let manual = ManualClock::new();
    let faces = crate::serve_fn_tests::faces(&manual);
    let (dir, socket, incoming) =
        crate::serve_tests::listen_as(move |io| Accepted::new(io, crate::serve_fn_tests::peer(role), role));
    let stop = Stop {
        signal: std::future::pending::<()>(),
        within: std::time::Duration::from_secs(30),
        clock: manual.clock(),
    };
    tokio::spawn(async move {
        let _kept = manual;
        serve(faces, routes, &[], incoming, stop).await
    });
    (dir, socket)
}

fn with_http() -> ProductRoutes {
    ProductRoutes::new().http(router()).expect("one router")
}

/// One HTTP/1 response read off `io`: its status line, its head and its body (by Content-Length), or None at EOF.
async fn response(io: &mut tokio::net::UnixStream) -> Option<(String, String, String)> {
    let mut seen = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = io.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        seen.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&seen[..head_end]).to_string();
    let length = head
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>()))
        .and_then(Result::ok)
        .unwrap_or(0);
    let mut body = seen[head_end + 4..].to_vec();
    while body.len() < length {
        let n = io.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let status = head.lines().next().unwrap_or_default().to_owned();
    Some((status, head, String::from_utf8_lossy(&body).to_string()))
}

/// Within a real-time bound, so a server that never answers fails the row rather than hanging it.
async fn within<T>(f: impl std::future::Future<Output = T>) -> T {
    anemnez_clock::Clock::system().timeout(std::time::Duration::from_secs(5), f).await.expect("answered in time")
}

/// F's (1) and (4): an HTTP/1 GET reaches the product's route carrying its connection's verdict, and a second request
/// rides the same kept-alive connection.
#[tokio::test(flavor = "multi_thread")]
async fn an_http1_request_reaches_the_products_route_with_its_verdict() {
    let (_dir, socket) = plane(Role::Other, with_http());
    let mut io = tokio::net::UnixStream::connect(&socket).await.expect("connects");
    for _ in 0..2 {
        io.write_all(b"GET /api/who HTTP/1.1\r\nHost: plane\r\n\r\n").await.expect("writes");
        let (status, _, body) = within(response(&mut io)).await.expect("an answer");
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        assert_eq!(body, "Some(Other)", "the handler reads the connection's Judged");
    }
}

/// D's (3): a refused HTTP request is a real status with the house-form refusal as its body, never an HTTP 200
/// carrying a grpc-status that a REST client would read as an empty success.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_http_request_is_403_with_its_refusal_never_200() {
    let (_dir, socket) = plane(Role::Fabricd, with_http());
    let mut io = tokio::net::UnixStream::connect(&socket).await.expect("connects");
    io.write_all(b"GET /api/who HTTP/1.1\r\nHost: plane\r\n\r\n").await.expect("writes");
    let (status, head, body) = within(response(&mut io)).await.expect("an answer");
    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    assert!(!head.to_ascii_lowercase().contains("grpc-status"), "no gRPC trailers on an HTTP refusal:\n{head}");
    assert!(body.starts_with(&format!("{FABRICD_READS_DESCRIBE_ONLY}: ")), "{body}");
}

/// D's (1): a plane whose product declares no HTTP routes stays HTTP/2 alone, closing an HTTP/1 request line at its
/// opening with no answer at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_plane_without_http_routes_closes_an_http1_opening() {
    let (_dir, socket) = plane(Role::Other, ProductRoutes::new());
    let mut io = tokio::net::UnixStream::connect(&socket).await.expect("connects");
    io.write_all(b"GET /api/who HTTP/1.1\r\nHost: plane\r\n\r\n").await.expect("writes");
    assert_eq!(within(response(&mut io)).await, None, "closed at the opening, never answered");
}

/// D's (4): a carried peer never opens HTTP/1, even on a plane with HTTP routes, so it reaches none of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_carried_peer_never_opens_http1() {
    let (_dir, socket) = plane(Role::Carried, with_http());
    let mut io = tokio::net::UnixStream::connect(&socket).await.expect("connects");
    io.write_all(b"GET /api/who HTTP/1.1\r\nHost: plane\r\n\r\n").await.expect("writes");
    assert_eq!(within(response(&mut io)).await, None, "closed at the opening");
}

/// F's (2) and (4): cargo's `Expect: 100-continue` is answered before the body is sent, and a chunked upload streams
/// to the product's handler whole.
#[tokio::test(flavor = "multi_thread")]
async fn an_expect_continue_upload_streams_to_the_handler() {
    let (_dir, socket) = plane(Role::Other, with_http());
    let mut io = tokio::net::UnixStream::connect(&socket).await.expect("connects");
    io.write_all(
        b"PUT /api/count HTTP/1.1\r\nHost: plane\r\nExpect: 100-continue\r\nTransfer-Encoding: chunked\r\n\r\n",
    )
    .await
    .expect("writes");
    let mut first = [0u8; 25];
    within(io.read_exact(&mut first)).await.expect("reads");
    assert!(first.starts_with(b"HTTP/1.1 100 Continue"), "{}", String::from_utf8_lossy(&first));
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..16 {
        io.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.expect("writes");
        io.write_all(&chunk).await.expect("writes");
        io.write_all(b"\r\n").await.expect("writes");
    }
    io.write_all(b"0\r\n\r\n").await.expect("writes");
    let (status, _, body) = within(response(&mut io)).await.expect("an answer");
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(body, (16 * 64 * 1024).to_string(), "every byte reached the handler");
}

/// D's (2): hyper's own parsing stands behind the opening. A request with both Content-Length and chunked
/// Transfer-Encoding, the smuggling shape, is read as chunked alone and its connection closed after it (RFC 9112
/// §6.3), so the request smuggled behind it never reaches a route.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_with_both_length_and_chunked_smuggles_nothing() {
    let (_dir, socket) = plane(Role::Other, with_http());
    let mut io = tokio::net::UnixStream::connect(&socket).await.expect("connects");
    io.write_all(
        b"PUT /api/count HTTP/1.1\r\nHost: plane\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\nGET /api/who HTTP/1.1\r\nHost: plane\r\n\r\n",
    )
    .await
    .expect("writes");
    let (status, head, body) = within(response(&mut io)).await.expect("an answer to the first");
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(body, "0", "read as chunked, its empty body, never four bytes of the next request");
    assert!(head.to_ascii_lowercase().contains("connection: close"), "the connection ends after it:\n{head}");
    assert_eq!(within(response(&mut io)).await, None, "the smuggled GET is never answered");
}

/// A product's HTTP routes are one router: a second is refused by name.
#[test]
fn a_second_http_router_is_refused_by_name() {
    let refused = with_http().http(router()).err().expect("refused");
    assert_eq!((refused.name, refused.service), (SERVICE_NAMED_TWICE, "http"));
}
