//! The live event stream: committed changes reach a subscribed device.

use std::time::Duration;

use futures::StreamExt;

use crate::phone_tests::serve;
use crate::session::SESSION_COOKIE;
use crate::test_support::signed_in_client;
use crate::test_support::test_server;

#[tokio::test]
async fn events_stream_follows_commits() {
    let t = test_server().await;
    let mut c = signed_in_client(&t).await;
    let base = serve(crate::router(t.server.clone())).await;
    let http = reqwest::Client::new();
    let cookie = format!("{SESSION_COOKIE}={}", c.cookie);
    let res = http
        .get(format!("{base}/api/me/events?account={}", c.account))
        .header("cookie", &cookie)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    let content_type = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    assert!(
        res.status() == 200 && content_type.starts_with("text/event-stream"),
        "events: {} {content_type}",
        res.status()
    );
    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    // Returns the next `data:` line, as frames arrive.
    let mut next = async || loop {
        if let Some(end) = buffer.find('\n') {
            let line: String = buffer.drain(..=end).collect();
            if let Some(data) = line.strip_prefix("data: ") {
                return data.trim().to_owned();
            }
            continue;
        }
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("a frame within five seconds")
            .expect("the stream stays open")
            .unwrap();
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    };
    assert_eq!(next().await, r#"{"seq":0}"#, "initial frame");
    c.must(
        "POST",
        "/api/me/sync",
        r#"{"since":0,"docs":[{"id":"t_a","type":"task","updated":1,"body":{"title":"A"}}]}"#,
        200,
    )
    .await;
    assert_eq!(next().await, r#"{"seq":1}"#, "commit frame");
    // A mismatched account in the query is refused like a mismatched header.
    let other =
        http.get(format!("{base}/api/me/events?account=someone-else")).header("cookie", &cookie).send().await.unwrap();
    assert_eq!(other.status(), 409, "stale account accepted");
}
