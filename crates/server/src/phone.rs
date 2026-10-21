//! Phone verification through Twilio Verify.
//!
//! Provider requests are never retried automatically: a timeout can follow
//! an accepted send or a consumed code. Phone numbers, codes, credentials,
//! and provider response bodies are never logged.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use regex::Regex;
use serde::Deserialize;

use crate::api::{ApiError, null_default, parse_json};
use crate::config::Twilio;

/// E.164 numbers only; the phone form handles country-specific parsing.
pub(crate) static PHONE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\+[1-9][0-9]{1,14}$").expect("valid pattern"));
/// Six-digit codes.
pub(crate) static CODE_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]{6}$").expect("valid pattern"));

/// Sends and checks phone codes. Errors that are [`PhoneError`] carry the
/// status and message shown to the student; any other error is reported as
/// 503 "Phone verification is unavailable. Try again shortly." (see
/// [`fail_phone`]). Tests install fakes with `Server::set_verifier`.
#[async_trait]
pub(crate) trait PhoneVerifier: Send + Sync {
    /// Sends a code to `phone`.
    async fn send(&self, phone: &str) -> anyhow::Result<()>;
    /// Checks `code` for `phone`; success means the number is verified.
    async fn check(&self, phone: &str, code: &str) -> anyhow::Result<()>;
}

/// A verification failure safe to show: HTTP status, the provider's error
/// code (logged, never shown), seconds for `Retry-After` (0 for none), and
/// the message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PhoneError {
    pub(crate) status: u16,
    pub(crate) code: i64,
    pub(crate) retry_after: u64,
    pub(crate) message: String,
}

impl std::fmt::Display for PhoneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PhoneError {}

/// Twilio Verify with one service. `base_url` is
/// `https://verify.twilio.com` except in tests.
pub(crate) struct TwilioVerify {
    pub(crate) cfg: Twilio,
    pub(crate) base_url: String,
    pub(crate) client: reqwest::Client,
}

impl TwilioVerify {
    /// A verifier for `cfg` at `base_url`, with a ten-second timeout and no
    /// redirects.
    pub(crate) fn new(cfg: Twilio, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the Twilio client configuration is valid");
        Self { cfg, base_url: base_url.trim_end_matches('/').to_owned(), client }
    }
}

#[async_trait]
impl PhoneVerifier for TwilioVerify {
    /// `POST Services/{sid}/Verifications` with `Channel=sms`; succeeds when
    /// Twilio reports `pending` for the same number.
    async fn send(&self, phone: &str) -> anyhow::Result<()> {
        Ok(self.request("Verifications", &[("To", phone), ("Channel", "sms")], "pending").await?)
    }

    /// `POST Services/{sid}/VerificationCheck`; succeeds on `approved`, and
    /// maps `pending`, 404, and Twilio error codes to [`PhoneError`]s.
    async fn check(&self, phone: &str, code: &str) -> anyhow::Result<()> {
        Ok(self.request("VerificationCheck", &[("To", phone), ("Code", code)], "approved").await?)
    }
}

/// The configured verifier: Twilio when all three credentials are set,
/// otherwise `None` (phone sign-in off).
pub(crate) fn new_phone_verifier(cfg: &Twilio) -> Option<Arc<dyn PhoneVerifier>> {
    if cfg.account_sid.is_empty() || cfg.auth_token.is_empty() || cfg.service_sid.is_empty() {
        return None;
    }
    Some(Arc::new(TwilioVerify::new(cfg.clone(), "https://verify.twilio.com")))
}

/// The response for a failed send or check: a [`PhoneError`]'s status,
/// message, and `Retry-After`, or 503 "Phone verification is unavailable.
/// Try again shortly." with `Retry-After: 30`. Logs the status and provider
/// code only.
pub(crate) fn fail_phone(err: anyhow::Error) -> ApiError {
    let e = err.downcast::<PhoneError>().unwrap_or_else(|_| PhoneError {
        status: 503,
        code: 0,
        retry_after: 30,
        message: "Phone verification is unavailable. Try again shortly.".into(),
    });
    // Never log the phone number, code, credentials, or provider response body.
    tracing::warn!(status = e.status, provider_code = e.code, "phone verification failed");
    let failure = ApiError::new(e.status, e.message);
    if e.retry_after > 0 { failure.retry_after(e.retry_after) } else { failure }
}

/// The largest provider reply read (32 KiB).
const REPLY_MAX: usize = 32 << 10;

/// The fields of a Twilio Verify reply the server reads.
#[derive(Default, Deserialize)]
#[serde(default)]
struct TwilioReply {
    #[serde(alias = "Status", deserialize_with = "null_default")]
    status: String,
    #[serde(alias = "To", deserialize_with = "null_default")]
    to: String,
    #[serde(alias = "Code", deserialize_with = "null_default")]
    code: i64,
}

impl TwilioVerify {
    /// Posts `form` to `Services/{sid}/{resource}` and succeeds when the
    /// reply names the same recipient with status `want`. Never retried.
    async fn request(&self, resource: &str, form: &[(&str, &str)], want: &str) -> Result<(), PhoneError> {
        let unavailable = PhoneError {
            status: 503,
            code: 0,
            retry_after: 30,
            message: "Phone verification is unavailable. Wait a moment, then request another code.".into(),
        };
        let endpoint = format!(
            "{}/v2/Services/{}/{resource}",
            self.base_url,
            utf8_percent_encode(&self.cfg.service_sid, PATH_SEGMENT)
        );
        let sent = self
            .client
            .post(endpoint)
            .basic_auth(&self.cfg.account_sid, Some(&self.cfg.auth_token))
            .form(form)
            .send()
            .await;
        let Ok(mut res) = sent else { return Err(unavailable) };
        let status = res.status().as_u16();
        let mut body = Vec::new();
        while body.len() < REPLY_MAX {
            match res.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(_) => return Err(unavailable),
            }
        }
        body.truncate(REPLY_MAX);
        // A JSON `null` reply reads as an empty one.
        let Some(reply) = parse_json::<Option<TwilioReply>>(&body) else { return Err(unavailable) };
        let out = reply.unwrap_or_default();
        let to = form.iter().find(|(k, _)| *k == "To").map(|(_, v)| *v).unwrap_or_default();
        if (200..300).contains(&status) {
            if out.to != to {
                return Err(unavailable);
            }
            if out.status == want {
                return Ok(());
            }
            if want == "approved" && out.status == "pending" {
                return Err(PhoneError {
                    status: 400,
                    message: "That code is incorrect. Try again.".into(),
                    ..Default::default()
                });
            }
            return Err(unavailable);
        }
        let code = out.code;
        let (status, retry_after, message) = if want == "approved" && status == 404 {
            // Twilio uses 404 for expired, exhausted, and already approved
            // codes. It is never evidence of successful verification.
            (400, 0, "This code has expired or was already used. Request another code.")
        } else if code == 60202 {
            (429, 600, "Too many incorrect codes. Wait ten minutes, then request another code.")
        } else if status == 429 || matches!(code, 60203 | 60212 | 60245) {
            (429, 600, "Too many codes requested. Try again in ten minutes.")
        } else if matches!(code, 60200 | 60205 | 21211 | 21614) {
            (400, 0, "Enter a valid mobile number that can receive text messages.")
        } else if matches!(code, 60410 | 60605 | 21408) {
            (400, 0, "We can't send a code to this number. Check the number or try again later.")
        } else {
            return Err(PhoneError { code, ..unavailable });
        };
        Err(PhoneError { status, code, retry_after, message: message.into() })
    }
}

/// Characters escaped in one URL path segment: everything but ASCII letters,
/// digits, and `-._~`.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
