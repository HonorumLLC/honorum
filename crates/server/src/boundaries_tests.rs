//! Outbound address checks at dial time, and forwarded client addresses,
//! which count only behind a trusted proxy.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{ConnectInfo, Request};

use crate::outbound::{Outbound, public_address};
use crate::test_support::{test_server, test_server_with};

#[tokio::test]
async fn outbound_rejects_local_address_at_dial() {
    for raw in [
        "127.0.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "10.2.3.4",
        "169.254.169.254",
        "fc00::1",
        "0.1.2.3",
        "100.64.0.1",
        "198.18.0.1",
        "64:ff9b::a02:304",
        "64:ff9b:1::1",
    ] {
        assert!(!public_address(raw.parse().unwrap()), "allowed {raw}");
    }
    for raw in ["8.8.8.8", "100.128.0.1", "2606:4700::1111"] {
        assert!(public_address(raw.parse().unwrap()), "refused {raw}");
    }
    // The client checks addresses when it connects: a name that resolves to
    // loopback is refused before any connection is made.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let seen = accepted.clone();
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    let outbound = Outbound::new();
    let result = outbound.client().get(format!("http://localhost:{port}/")).send().await;
    assert!(result.is_err(), "connected to loopback");
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "connected to loopback");
}

fn request_from(remote: &str) -> Request {
    let mut r =
        Request::builder().uri("/").header("cf-connecting-ip", "198.51.100.4").body(axum::body::Body::empty()).unwrap();
    r.extensions_mut().insert(ConnectInfo(remote.parse::<SocketAddr>().unwrap()));
    r
}

#[tokio::test]
async fn forwarded_addresses_require_trust() {
    let t = test_server().await;
    assert_eq!(t.server.client_ip(&request_from("192.0.2.1:1234")), "192.0.2.1");
    let trusted = test_server_with(|cfg| cfg.trust_proxy = true).await;
    assert_eq!(trusted.server.client_ip(&request_from("192.0.2.1:1234")), "198.51.100.4");
}
