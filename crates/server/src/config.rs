//! Service configuration: the JSON file for schools and defaults, and the
//! environment for secrets. See `docs/deployment.md` for every setting.

use std::path::Path;

use serde::Deserialize;

use crate::closures::Closure;

/// Twilio Verify credentials, read only from the environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Twilio {
    pub(crate) account_sid: String,
    pub(crate) auth_token: String,
    pub(crate) service_sid: String,
}

/// Apple Push Notification service credentials, from the environment.
#[derive(Clone, Debug, Default)]
pub(crate) struct Apns {
    /// The `.p8` signing key, in PEM.
    pub(crate) key: String,
    pub(crate) key_id: String,
    pub(crate) team_id: String,
    /// The app's bundle identifier.
    pub(crate) topic: String,
    pub(crate) sandbox: bool,
}

/// Google Calendar's OAuth client, from the environment.
#[derive(Clone, Debug, Default)]
pub(crate) struct GoogleOauth {
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
}

/// The school's learning management system, so the Tasks connection flow
/// can prefill it. `kind` is `canvas`, `blackboard`, `brightspace`, or
/// `moodle`; `url` is the site students sign in to.
#[derive(Clone, Debug, Default, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct Lms {
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) url: String,
}

/// One configured school.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct InstitutionConfig {
    #[serde(default)]
    pub(crate) poll_seconds: i64,
    pub id: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub name: String,
    /// The adapter module that collects this school's catalog.
    #[serde(default)]
    pub adapter: String,
    #[serde(default)]
    pub url: String,
    /// The provider environment, such as Civitas's
    /// `<env>.search.collegescheduler.com` subdomain or a Rutgers campus.
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub lms: Option<Lms>,
    /// Replaces the built-in list of days classes do not meet.
    #[serde(default)]
    pub closures: Option<Vec<Closure>>,
}

/// The whole configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Base64 AES-256 key that seals account credentials; kept apart from the
    /// database and its backups.
    #[serde(skip)]
    pub(crate) canvas_key: String,
    #[serde(skip)]
    pub(crate) apns: Apns,
    pub(crate) trust_proxy: bool,
    pub base_url: String,
    /// A `mailto:` address, used as the VAPID subject and in the user agent.
    pub contact: String,
    pub(crate) poll_seconds: i64,
    pub(crate) catalog_hours: i64,
    pub(crate) archive_catalog_hours: i64,
    pub(crate) debounce_minutes: i64,
    #[serde(skip)]
    pub(crate) twilio: Twilio,
    #[serde(skip)]
    pub(crate) google: GoogleOauth,
    /// Opens `/admin`; the dashboard is off without it.
    #[serde(skip)]
    pub(crate) admin_token: String,
    /// VAPID keys sign every push message. Left empty, a pair is generated on
    /// first start and kept in the database; set both to survive a lost
    /// database without invalidating every browser's subscription.
    pub(crate) vapid_public: String,
    pub(crate) vapid_private: String,
    pub institutions: Vec<InstitutionConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            canvas_key: String::new(),
            apns: Apns::default(),
            trust_proxy: false,
            base_url: "http://localhost:8080".into(),
            contact: "mailto:support@honorum.app".into(),
            poll_seconds: 120,
            catalog_hours: 6,
            archive_catalog_hours: 168,
            debounce_minutes: 30,
            twilio: Twilio::default(),
            google: GoogleOauth::default(),
            admin_token: String::new(),
            vapid_public: String::new(),
            vapid_private: String::new(),
            institutions: Vec::new(),
        }
    }
}

impl Config {
    /// Reads the file at `path`, then the environment.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let mut c: Config = serde_json::from_slice(&raw).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if c.poll_seconds < 10
            || c.catalog_hours < 1
            || c.archive_catalog_hours < c.catalog_hours
            || c.debounce_minutes < 0
        {
            anyhow::bail!(
                "poll_seconds must be at least 10, catalog_hours at least 1, archive_catalog_hours at least catalog_hours, and debounce_minutes nonnegative"
            );
        }
        c.apply_env(|key| std::env::var(key).unwrap_or_default())?;
        for inst in &c.institutions {
            if inst.poll_seconds != 0 && inst.poll_seconds < 10 {
                anyhow::bail!("{}: poll_seconds must be at least 10", inst.id);
            }
            if !inst.timezone.is_empty() && inst.timezone.parse::<chrono_tz::Tz>().is_err() {
                anyhow::bail!("{}: bad institution timezone: unknown time zone {}", inst.id, inst.timezone);
            }
        }
        Ok(c)
    }

    /// Reads secrets and overrides from the environment.
    fn apply_env(&mut self, env: impl Fn(&str) -> String) -> anyhow::Result<()> {
        self.canvas_key = env("HONORUM_CANVAS_KEY");
        self.twilio = Twilio {
            account_sid: env("HONORUM_TWILIO_ACCOUNT_SID"),
            auth_token: env("HONORUM_TWILIO_AUTH_TOKEN"),
            service_sid: env("HONORUM_TWILIO_VERIFY_SERVICE_SID"),
        };
        if self.twilio != Twilio::default()
            && (self.twilio.account_sid.is_empty()
                || self.twilio.auth_token.is_empty()
                || self.twilio.service_sid.is_empty())
        {
            anyhow::bail!(
                "phone sign-in requires HONORUM_TWILIO_ACCOUNT_SID, HONORUM_TWILIO_AUTH_TOKEN, and HONORUM_TWILIO_VERIFY_SERVICE_SID"
            );
        }
        self.google = GoogleOauth {
            client_id: env("HONORUM_GOOGLE_CLIENT_ID"),
            client_secret: env("HONORUM_GOOGLE_CLIENT_SECRET"),
        };
        if self.google.client_id.is_empty() != self.google.client_secret.is_empty() {
            anyhow::bail!("Google Calendar requires both HONORUM_GOOGLE_CLIENT_ID and HONORUM_GOOGLE_CLIENT_SECRET");
        }
        self.admin_token = env("HONORUM_ADMIN_TOKEN");
        if !self.admin_token.is_empty() && self.admin_token.len() < 32 {
            anyhow::bail!("HONORUM_ADMIN_TOKEN must be at least 32 characters");
        }
        let base = env("HONORUM_BASE_URL");
        if !base.is_empty() {
            self.base_url = base;
        }
        self.apns = Apns {
            key: env("HONORUM_APNS_KEY"),
            key_id: env("HONORUM_APNS_KEY_ID"),
            team_id: env("HONORUM_APNS_TEAM_ID"),
            topic: env("HONORUM_APNS_TOPIC"),
            sandbox: env("HONORUM_APNS_SANDBOX") == "1",
        };
        let key_file = env("HONORUM_APNS_KEY_FILE");
        if !key_file.is_empty() && self.apns.key.is_empty() {
            self.apns.key =
                std::fs::read_to_string(&key_file).map_err(|e| anyhow::anyhow!("HONORUM_APNS_KEY_FILE: {e}"))?;
        }
        let (public, private) = (env("HONORUM_VAPID_PUBLIC"), env("HONORUM_VAPID_PRIVATE"));
        if !public.is_empty() && !private.is_empty() {
            self.vapid_public = public;
            self.vapid_private = private;
        }
        if env("HONORUM_TRUST_PROXY") == "1" {
            self.trust_proxy = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_twilio_credentials_are_refused() {
        let mut c = Config::default();
        let err =
            c.apply_env(|k| if k == "HONORUM_TWILIO_ACCOUNT_SID" { "AC1".into() } else { String::new() }).unwrap_err();
        assert!(err.to_string().contains("phone sign-in requires"));
    }

    #[test]
    fn short_admin_tokens_are_refused() {
        let mut c = Config::default();
        assert!(c.apply_env(|k| if k == "HONORUM_ADMIN_TOKEN" { "short".into() } else { String::new() }).is_err());
    }
}
