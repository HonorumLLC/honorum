//! Days when classes do not meet.
//!
//! Each registrar's calendar is transcribed once a year into
//! `closures.json`. A closure may instead say the date follows another
//! weekday's schedule (`follows`: one of `M T W R F`), or cancel only the
//! classes that start at or after a campus time (`after`, in minutes after
//! midnight, like meeting times).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closure {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) follows: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) after: i64,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

impl Closure {
    /// Reports whether the closure cancels a class meeting that starts at
    /// `start` minutes after midnight. A substitute weekday cancels the
    /// date's own classes.
    pub(crate) fn cancels(&self, start: i64) -> bool {
        self.after == 0 || start >= self.after
    }
}

const BUILT_IN: &str = include_str!("closures.json");

/// The built-in closures, by institution ID.
pub(crate) fn load_closures() -> anyhow::Result<HashMap<String, Vec<Closure>>> {
    #[derive(Deserialize)]
    struct Entry {
        #[serde(default)]
        days: Vec<Closure>,
    }
    let raw: HashMap<String, serde_json::Value> =
        serde_json::from_str(BUILT_IN).map_err(|e| anyhow::anyhow!("closures.json: {e}"))?;
    let mut out = HashMap::new();
    for (id, value) in raw {
        if id == "_comment" {
            continue;
        }
        let entry: Entry = serde_json::from_value(value).map_err(|e| anyhow::anyhow!("closures.json {id}: {e}"))?;
        for d in &entry.days {
            if d.from.len() != 10
                || d.to.len() != 10
                || d.to < d.from
                || d.name.is_empty()
                || !(0..1440).contains(&d.after)
                || (d.after > 0 && !d.follows.is_empty())
            {
                anyhow::bail!("closures.json {id}: bad entry {d:?}");
            }
        }
        out.insert(id, entry.days);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_closures_load() {
        let all = load_closures().unwrap();
        assert!(all.contains_key("ou"), "OU closures are built in");
    }

    #[test]
    fn partial_closures_cancel_later_classes() {
        let c = Closure { after: 900, ..Default::default() };
        assert!(!c.cancels(899));
        assert!(c.cancels(900));
        assert!(Closure::default().cancels(0));
    }
}
