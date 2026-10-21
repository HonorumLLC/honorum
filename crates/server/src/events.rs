//! Live account events: `GET /api/me/events` streams the account's document
//! sequence (and Google Calendar version) as server-sent events, and one
//! PostgreSQL `LISTEN honorum_accounts` connection per API process relays
//! commits from other replicas into the in-process [`crate::hub::Hub`].
//! Each API process reads heartbeats for many streams together.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Response, header};
use bytes::Bytes;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::api::ApiResult;
use crate::db::Sql;
use crate::hub::Wakeup;
use crate::session::{Auth, Session};

/// Seconds between heartbeat frames.
pub(crate) const HEARTBEAT_SECONDS: u64 = 25;
/// Streams one heartbeat read covers at most.
const HEARTBEAT_BATCH: usize = 1000;

/// `GET /api/me/events`: an SSE stream of `event: seq` frames
/// (`{"seq": n}`, plus `"google": v` when Google Calendar is connected),
/// sent at once, after each wakeup, and every 25 seconds. The stream ends
/// when the session is deleted, a read fails, or the client leaves.
/// Browsers pass the expected account as `?account=` (the router moves it
/// into `X-Honorum-Account`).
///
/// Each frame is written as soon as it is read. The stream holds no pool
/// connection between frames; the heartbeat read of the durable sequence
/// also covers notifications missed while a listener reconnected.
/// Heartbeats come from [`Server::run_events`], which reads them for many
/// streams at once; a stream that hears none for two periods reads its own.
pub(crate) async fn handle_events(State(s): State<Arc<Server>>, Auth(se): Auth) -> ApiResult {
    // One frame in flight: a slow client delays reads rather than queueing.
    let (frames, mut receiver) = mpsc::channel::<Bytes>(1);
    tokio::spawn(stream_events(s, se, frames));
    let body = futures::stream::poll_fn(move |cx| receiver.poll_recv(cx).map(|f| f.map(Ok::<_, Infallible>)));
    let mut response = Response::new(Body::from_stream(body));
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    Ok(response)
}

/// Writes frames to `frames` until the session ends, a read fails, or the
/// receiver (the response body) is dropped.
async fn stream_events(s: Arc<Server>, se: Session, frames: mpsc::Sender<Bytes>) {
    let sub = s.events.subscribe(&se.account, &se.id);
    if !send_frame(&s, &se, &frames).await {
        return;
    }
    let quiet = Duration::from_secs(2 * HEARTBEAT_SECONDS);
    loop {
        let sent = tokio::select! {
            _ = frames.closed() => return,
            wake = sub.woken() => match wake {
                Wakeup::Changed => send_frame(&s, &se, &frames).await,
                Wakeup::Beat(Some((seq, google))) => send(&frames, seq, google).await,
                Wakeup::Beat(None) => return,
            },
            _ = tokio::time::sleep(quiet) => send_frame(&s, &se, &frames).await,
        };
        if !sent {
            return;
        }
    }
}

/// Reads and sends one frame; false when the stream should end.
async fn send_frame(s: &Server, se: &Session, frames: &mpsc::Sender<Bytes>) -> bool {
    let row = s
        .db
        .fetch_one(
            "SELECT a.seq,(SELECT COUNT(*) FROM sessions WHERE id=$1),COALESCE((SELECT version FROM google_connections WHERE account=a.id),0) FROM accounts a WHERE a.id=$2",
            &[&se.id, &se.account],
        )
        .await;
    let read =
        row.and_then(|row| Ok((row.try_get::<_, i64>(0)?, row.try_get::<_, i64>(1)?, row.try_get::<_, i64>(2)?)));
    let Ok((seq, alive, google)) = read else { return false };
    if alive == 0 {
        return false;
    }
    send(frames, seq, google).await
}

/// Sends one frame; false when the client left.
async fn send(frames: &mpsc::Sender<Bytes>, seq: i64, google: i64) -> bool {
    let mut frame = Map::new();
    frame.insert("seq".into(), Value::from(seq));
    // With Google Calendar connected, google is the version of its events,
    // so devices fetch them again when it moves.
    if google > 0 {
        frame.insert("google".into(), Value::from(google));
    }
    let data = serde_json::to_string(&frame).expect("frames serialize");
    frames.send(Bytes::from(format!("event: seq\ndata: {data}\n\n"))).await.is_ok()
}

impl Server {
    /// Relays account notifications to local streams ([`Server::relay_events`])
    /// and sends them heartbeats every 25 seconds, until `stop` fires.
    pub async fn run_events(self: &Arc<Self>, stop: CancellationToken) {
        tokio::join!(self.relay_events(&stop), self.run_heartbeats(&stop));
    }

    /// Reads each local stream's heartbeat every 25 seconds, in batches of
    /// [`HEARTBEAT_BATCH`] streams, until `stop` fires. Streams take turns
    /// across the period's seconds, so frames go out evenly rather than all
    /// at once. A batch that cannot be read is skipped; its streams read
    /// their own after two periods.
    async fn run_heartbeats(&self, stop: &CancellationToken) {
        let second = Duration::from_secs(1);
        let mut ticker = tokio::time::interval_at(Instant::now() + second, second);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        for tick in 0.. {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = ticker.tick() => {}
            }
            for batch in self.events.slots(tick, HEARTBEAT_SECONDS).chunks(HEARTBEAT_BATCH) {
                if let Err(err) = self.heartbeat(batch).await {
                    tracing::warn!(%err, "event heartbeats could not be read");
                }
            }
        }
    }

    /// Reads one batch of heartbeats and hands each stream its own.
    pub(crate) async fn heartbeat(&self, batch: &[(String, Arc<crate::hub::Slot>)]) -> crate::db::Result<()> {
        let accounts: Vec<&str> = batch.iter().map(|(a, _)| a.as_str()).collect::<HashSet<_>>().into_iter().collect();
        let sessions: Vec<&str> = batch.iter().map(|(_, slot)| slot.session()).collect();
        let rows = self
            .db
            .fetch_all(
                "SELECT a.id,a.seq,COALESCE((SELECT version FROM google_connections WHERE account=a.id),0) FROM accounts a WHERE a.id=ANY($1::text[])",
                &[&accounts],
            )
            .await?;
        let mut current = HashMap::new();
        for row in &rows {
            current.insert(row.try_get::<_, String>(0)?, (row.try_get::<_, i64>(1)?, row.try_get::<_, i64>(2)?));
        }
        let rows = self.db.fetch_all("SELECT id FROM sessions WHERE id=ANY($1::text[])", &[&sessions]).await?;
        let mut alive = HashSet::new();
        for row in &rows {
            alive.insert(row.try_get::<_, String>(0)?);
        }
        for (account, slot) in batch {
            let beat = current.get(account).copied().filter(|_| alive.contains(slot.session()));
            slot.beat(beat);
        }
        Ok(())
    }

    /// Keeps one `LISTEN honorum_accounts` connection open, waking local
    /// streams for each notified account and every local account after a
    /// reconnect; reconnects a second after a failure, until `stop` fires.
    /// Notifications carry only an account ID and are hints; sync and
    /// heartbeat reads remain the source of truth.
    async fn relay_events(&self, stop: &CancellationToken) {
        while !stop.is_cancelled() {
            let err = self.listen_events(stop).await;
            if stop.is_cancelled() {
                return;
            }
            let err = err.err().map(|e| e.to_string()).unwrap_or_default();
            tracing::warn!(%err, "account event listener will reconnect");
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    /// Listens until `stop` fires (`Ok`) or the connection fails.
    async fn listen_events(&self, stop: &CancellationToken) -> anyhow::Result<()> {
        let (_client, mut notes) = tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            listening = self.db.listen("honorum_accounts") => listening?,
        };
        // Wake every local account after a reconnect; events committed while
        // disconnected are recovered from the database.
        for account in self.events.accounts() {
            self.events.notify(&account);
        }
        loop {
            tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                note = notes.recv() => match note {
                    Some(note) => self.events.notify(note.payload()),
                    None => anyhow::bail!("the listener connection closed"),
                },
            }
        }
    }
}
