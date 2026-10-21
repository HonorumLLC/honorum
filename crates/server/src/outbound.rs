//! Requests to addresses a student supplies: Canvas sites, calendar feeds,
//! and push services.
//!
//! The client resolves each host itself and refuses to connect when any
//! address is not a public one, closing the gap where a name resolves to a
//! public address when checked and a private one when dialed. A URL with a
//! literal address skips resolution, so every request URL must first pass
//! [`Outbound::safe_url`], which refuses literal non-public addresses. The
//! client ignores proxy settings and follows no redirects itself: the
//! caller checks and follows each hop (see `lms::relay_raw`).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// The outbound client and its test switch.
#[derive(Clone)]
pub(crate) struct Outbound {
    /// Lets tests point imports at a loopback server.
    allow_local: Arc<AtomicBool>,
    client: reqwest::Client,
}

impl Default for Outbound {
    fn default() -> Self {
        Self::new()
    }
}

impl Outbound {
    pub(crate) fn new() -> Self {
        let allow_local = Arc::new(AtomicBool::new(false));
        let client = reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(PublicResolver { allow_local: allow_local.clone() }))
            .timeout(Duration::from_secs(25))
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the outbound client configuration is valid");
        Self { allow_local, client }
    }

    /// The client. Requests time out after 25 seconds and do not follow
    /// redirects.
    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn allow_local(&self) -> bool {
        self.allow_local.load(Ordering::Relaxed)
    }

    /// Tests and labs call this to reach a loopback server over plain HTTP.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn set_allow_local(&self, allow: bool) {
        self.allow_local.store(allow, Ordering::Relaxed);
    }

    /// Parses a student-supplied URL: HTTPS only, with a host and no
    /// credentials, and never a literal non-public address.
    pub(crate) fn safe_url(&self, raw: &str) -> Result<url::Url, &'static str> {
        check_url(raw, self.allow_local())
    }
}

fn check_url(raw: &str, allow_local: bool) -> Result<url::Url, &'static str> {
    let u = url::Url::parse(raw.trim()).map_err(|_| "bad url")?;
    let host = u.host_str().unwrap_or_default();
    if host.is_empty() || !u.username().is_empty() || u.password().is_some() {
        return Err("bad url");
    }
    if u.scheme() != "https" && !(allow_local && u.scheme() == "http") {
        return Err("only https urls are allowed");
    }
    if let Some(ip) = literal_ip(&u)
        && !allow_local
        && !public_address(ip)
    {
        return Err("host not allowed");
    }
    Ok(u)
}

fn literal_ip(u: &url::Url) -> Option<IpAddr> {
    match u.host()? {
        url::Host::Ipv4(v4) => Some(IpAddr::V4(v4)),
        url::Host::Ipv6(v6) => Some(IpAddr::V6(v6)),
        url::Host::Domain(_) => None,
    }
}

/// Reports whether `a` and `b` share a scheme, host, and port, treating a
/// missing port as the scheme's default.
pub(crate) fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme().eq_ignore_ascii_case(b.scheme())
        && a.host_str().unwrap_or_default().eq_ignore_ascii_case(b.host_str().unwrap_or_default())
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Reports whether `ip` names a public server. Besides private, loopback,
/// and link-local ranges, this refuses "this network", carrier-grade NAT,
/// benchmarking, and the NAT64 prefixes, which can reach any IPv4 address.
pub(crate) fn public_address(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19)))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let unique_local = (s[0] & 0xfe00) == 0xfc00;
            let link_local = (s[0] & 0xffc0) == 0xfe80;
            let nat64 = (s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0])
                || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1);
            !(v6.is_unspecified() || v6.is_loopback() || v6.is_multicast() || unique_local || link_local || nat64)
        }
    }
}

struct PublicResolver {
    allow_local: Arc<AtomicBool>,
}

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_local = self.allow_local.load(Ordering::Relaxed);
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if !allow_local && addresses.iter().any(|a| !public_address(a.ip())) {
                return Err("destination is not a public address".into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses_exclude_special_ranges() {
        for private in [
            "10.0.0.1",
            "127.0.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "0.1.2.3",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "64:ff9b::a00:1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!public_address(private.parse().unwrap()), "{private}");
        }
        for public in ["8.8.8.8", "2606:4700::1111", "100.128.0.1"] {
            assert!(public_address(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn safe_urls_need_https_and_a_public_host() {
        let out = Outbound::new();
        assert!(out.safe_url("https://canvas.example.edu/").is_ok());
        assert_eq!(out.safe_url("http://canvas.example.edu/"), Err("only https urls are allowed"));
        assert_eq!(out.safe_url("https://10.0.0.1/"), Err("host not allowed"));
        assert_eq!(out.safe_url("https://user:pw@example.edu/"), Err("bad url"));
    }
}
