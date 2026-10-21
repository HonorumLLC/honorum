//! The HTTP client adapters share. Requests to one host go one at a time,
//! spaced at least an interval apart, so every adapter shares one politeness
//! budget per origin.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use reqwest::{RequestBuilder, StatusCode};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// A provider request failure. Messages name the status or transport error
/// and never include response bodies.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("{0}")]
    Transport(#[from] reqwest::Error),
    #[error("upstream returned {0}")]
    Status(StatusCode),
}

/// A paced client. Cloning shares the pacing gates.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    user_agent: Arc<str>,
    interval: Duration,
    gates: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl Http {
    /// Builds a client that identifies itself with `user_agent` and spaces
    /// requests to one host at least `interval` apart.
    pub fn new(user_agent: impl Into<String>, interval: Duration) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("the HTTP client configuration is valid");
        Self { client, user_agent: user_agent.into().into(), interval, gates: Default::default() }
    }

    /// An unpaced client for tests and fixtures.
    pub fn unpaced() -> Self {
        Self::new("Honorum", Duration::ZERO)
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// Starts a request with the shared user agent.
    pub fn get(&self, url: &str) -> RequestBuilder {
        self.client.get(url).header(reqwest::header::USER_AGENT, &*self.user_agent)
    }

    /// Starts a request with the shared user agent.
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.client.post(url).header(reqwest::header::USER_AGENT, &*self.user_agent)
    }

    /// Sends a request through its host's gate and reads the whole body.
    /// A status other than 2xx is an error.
    pub async fn send(&self, request: RequestBuilder, timeout: Duration) -> Result<Bytes, HttpError> {
        let (status, body) = self.send_any(request, timeout).await?;
        if !status.is_success() {
            return Err(HttpError::Status(status));
        }
        Ok(body)
    }

    /// Sends a request through its host's gate and returns any status with
    /// the body.
    pub async fn send_any(&self, request: RequestBuilder, timeout: Duration) -> Result<(StatusCode, Bytes), HttpError> {
        let (status, _, body) = self.send_full(request, timeout).await?;
        Ok((status, body))
    }

    /// Sends a request through its host's gate and returns any status with
    /// the response headers and body. Adapters that keep a provider session
    /// read `Set-Cookie` from the headers.
    pub async fn send_full(
        &self,
        request: RequestBuilder,
        timeout: Duration,
    ) -> Result<(StatusCode, reqwest::header::HeaderMap, Bytes), HttpError> {
        let request = request.timeout(timeout).build()?;
        let host = request.url().host_str().unwrap_or_default().to_owned();
        let permit = self.gate(&host).acquire_owned().await.expect("gates are never closed");
        let started = Instant::now();
        let result = async {
            let response = self.client.execute(request).await?;
            let status = response.status();
            let headers = response.headers().clone();
            Ok((status, headers, response.bytes().await?))
        }
        .await;
        self.release(permit, started);
        result
    }

    fn gate(&self, host: &str) -> Arc<Semaphore> {
        let mut gates = self.gates.lock().expect("gate map lock");
        gates.entry(host.to_owned()).or_insert_with(|| Arc::new(Semaphore::new(1))).clone()
    }

    fn release(&self, permit: OwnedSemaphorePermit, started: Instant) {
        let remaining = self.interval.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            drop(permit);
        } else {
            tokio::spawn(async move {
                tokio::time::sleep(remaining).await;
                drop(permit);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn a_cancelled_wait_releases_nothing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    let _ = socket.read(&mut buf).await;
                    let _ =
                        socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok").await;
                });
            }
        });
        let http = Http::new("test", Duration::from_millis(200));
        let url = format!("http://{addr}/");
        http.send(http.get(&url), Duration::from_secs(1)).await.unwrap();
        // The gate is held for the interval; a dropped wait must not take it.
        let waiting =
            tokio::time::timeout(Duration::from_millis(20), http.send(http.get(&url), Duration::from_secs(1))).await;
        assert!(waiting.is_err(), "the paced request did not wait");
        let next = http.send(http.get(&url), Duration::from_secs(1)).await.unwrap();
        assert_eq!(&next[..], b"ok");
    }
}
