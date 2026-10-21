//! Development labs run by the `honorum-lab` binary: the server behind the
//! web browser suite (`browser`), the account API for
//! `apps/web/scripts/perf.mjs` (`perf`), and the capacity lab driven by
//! `scripts/load-test.py` (`capacity`). Nothing here is compiled into the
//! application; the module exists only with the `lab` feature.

pub mod browser;
mod canvas;
pub mod capacity;
pub mod perf;

use std::time::Duration;

/// Parses a duration string such as `300ms`, `1.5s`, or `1m30s`: an
/// optional sign followed by decimal numbers, each with a unit of `ns`,
/// `us` (or `µs`), `ms`, `s`, `m`, or `h`. `0` alone is zero. Negative
/// durations and malformed input yield `None`.
pub(crate) fn parse_duration(raw: &str) -> Option<Duration> {
    let mut s = raw.strip_prefix('+').unwrap_or(raw);
    if s == "0" {
        return Some(Duration::ZERO);
    }
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let mut total = 0f64;
    while !s.is_empty() {
        let digits = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
        let number: f64 = s[..digits].parse().ok()?;
        s = &s[digits..];
        let unit_len = s.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(s.len());
        let scale = match &s[..unit_len] {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => return None,
        };
        s = &s[unit_len..];
        total += number * scale;
    }
    Some(Duration::from_nanos(total.round() as u64))
}

/// Formats a duration in the form [`parse_duration`] reads: `0s`, `300ms`,
/// `1.5s`, `1m30s`, `2h0m0s`.
pub(crate) fn format_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return "0s".into();
    }
    let trim = |whole: u128, frac: u128, width: usize| {
        let mut out = whole.to_string();
        if frac > 0 {
            let digits = format!("{frac:0width$}");
            out.push('.');
            out.push_str(digits.trim_end_matches('0'));
        }
        out
    };
    if nanos < 1_000 {
        return format!("{nanos}ns");
    }
    if nanos < 1_000_000 {
        return format!("{}µs", trim(nanos / 1_000, nanos % 1_000, 3));
    }
    if nanos < 1_000_000_000 {
        return format!("{}ms", trim(nanos / 1_000_000, nanos % 1_000_000, 6));
    }
    let (hours, rest) = (nanos / 3_600_000_000_000, nanos % 3_600_000_000_000);
    let (minutes, rest) = (rest / 60_000_000_000, rest % 60_000_000_000);
    let seconds = trim(rest / 1_000_000_000, rest % 1_000_000_000, 9);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, m) => format!("{m}m{seconds}s"),
        (h, m) => format!("{h}h{m}m{seconds}s"),
    }
}

/// Resolves on SIGINT or SIGTERM.
pub(crate) async fn shutdown_signal() {
    let mut terminate =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("a SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("300ms"), Some(Duration::from_millis(300)));
        assert_eq!(parse_duration("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1.5s"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_duration("0"), Some(Duration::ZERO));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("5"), None);
        assert_eq!(format_duration(Duration::ZERO), "0s");
        assert_eq!(format_duration(Duration::from_millis(300)), "300ms");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h0m0s");
    }
}
