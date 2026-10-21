//! Push notifications: the alert payload, delivery to one destination (a
//! Web Push subscription or an APNs device token), and reading stored
//! destinations.
//!
//! Delivery goes through [`Server::send_alert`], which uses a replacement
//! [`Sender`] when one is installed (tests) and [`Server::deliver`]
//! otherwise.

pub(crate) mod apns;
pub(crate) mod webpush;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Server;

/// What the service worker or iOS shows; also the stored payload of a
/// queued delivery.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Alert {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) url: String,
}

/// The result of one send: whether the destination is permanently invalid
/// and should be forgotten (`gone`), and whether this send failed. Both can
/// be true at once, as when a subscription cannot be parsed.
pub(crate) type SendOutcome = (bool, anyhow::Result<()>);

/// A replacement for push delivery, for tests and labs. Closures `Fn(&str,
/// &Alert) -> SendOutcome` implement it.
#[async_trait]
pub(crate) trait Sender: Send + Sync {
    /// Sends `alert` to the stored destination `target`.
    async fn send(&self, target: &str, alert: &Alert) -> SendOutcome;
}

#[async_trait]
impl<F> Sender for F
where
    F: Fn(&str, &Alert) -> SendOutcome + Send + Sync,
{
    async fn send(&self, target: &str, alert: &Alert) -> SendOutcome {
        self(target, alert)
    }
}

/// A stored push destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PushTarget {
    /// A lowercase APNs device token, stored as `{"apns":"<token>"}`.
    Apns(String),
    /// A Web Push subscription's https endpoint.
    Web(String),
}

/// Reads a stored destination: an APNs token matching
/// [`apns::APNS_TOKEN`], or a Web Push subscription with an https
/// endpoint; `None` otherwise.
pub(crate) fn push_target(target: &str) -> Option<PushTarget> {
    let fields = string_fields(target, &["endpoint", "apns"])?;
    let (endpoint, apns) = (&fields[0], &fields[1]);
    if !apns.is_empty() && apns::APNS_TOKEN.is_match(apns) {
        return Some(PushTarget::Apns(apns.to_lowercase()));
    }
    if endpoint.starts_with("https://") {
        return Some(PushTarget::Web(endpoint.clone()));
    }
    None
}

/// The https endpoint of a serialized Web Push subscription, or empty.
pub(crate) fn push_endpoint(target: &str) -> String {
    match string_fields(target, &["endpoint"]) {
        Some(mut fields) if fields[0].starts_with("https://") => fields.swap_remove(0),
        _ => String::new(),
    }
}

/// Reads string fields of a JSON object: names match exactly or else
/// case-insensitively, a missing or
/// `null` field is empty, and `None` means the text is not JSON, not an
/// object (or `null`), or a named field is not a string.
fn string_fields(text: &str, names: &[&str]) -> Option<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = match value {
        serde_json::Value::Object(object) => object,
        serde_json::Value::Null => return Some(vec![String::new(); names.len()]),
        _ => return None,
    };
    names
        .iter()
        .map(|name| {
            let field =
                object.get(*name).or_else(|| object.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v));
            match field {
                None | Some(serde_json::Value::Null) => Some(String::new()),
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(_) => None,
            }
        })
        .collect()
}

impl Server {
    /// Sends one alert through the installed [`Sender`], or
    /// [`Server::deliver`] when none is installed.
    pub(crate) async fn send_alert(&self, target: &str, alert: &Alert) -> SendOutcome {
        let sender = self.sender.read().expect("sender lock").clone();
        match sender {
            Some(sender) => sender.send(target, alert).await,
            None => self.deliver(target, alert).await,
        }
    }

    /// Delivers to an APNs token ("APNs is not configured on this server"
    /// without a sender) or a Web Push subscription (VAPID-signed, TTL one
    /// hour). 404/410 mark the destination gone; 401/403 mean a
    /// subscription bound to another VAPID key; other statuses of 400 and
    /// above fail.
    pub(crate) async fn deliver(&self, target: &str, alert: &Alert) -> SendOutcome {
        if let Some(PushTarget::Apns(token)) = push_target(target) {
            let Some(sender) = self.apns() else {
                return (false, Err(anyhow::anyhow!("APNs is not configured on this server")));
            };
            return sender.send(&token, alert, self.now()).await;
        }
        let subscription: webpush::Subscription = match serde_json::from_str(target) {
            Ok(subscription) => subscription,
            Err(err) => return (true, Err(err.into())),
        };
        let payload = serde_json::to_vec(alert).expect("alerts serialize");
        let status = match webpush::send_notification(
            self.outbound.client(),
            &payload,
            &subscription,
            &self.cfg.contact,
            &self.vapid,
            3600,
        )
        .await
        {
            Ok(status) => status,
            Err(err) => return (false, Err(err)),
        };
        match status {
            404 | 410 => (true, Err(anyhow::anyhow!("push endpoint gone: {status}"))),
            // The push service rejects a VAPID signature that does not match
            // the key the browser subscribed with; the browser resubscribes on
            // its next visit and reports the new subscription.
            401 | 403 => {
                (false, Err(anyhow::anyhow!("push rejected (subscription bound to another VAPID key?): {status}")))
            }
            400.. => (false, Err(anyhow::anyhow!("push failed: {status}"))),
            _ => (false, Ok(())),
        }
    }
}
