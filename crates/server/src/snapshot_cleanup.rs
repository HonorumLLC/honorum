//! Removing superseded snapshot objects from the data directory. The
//! current generation of each kind, the newest previous one, and any
//! modified within the last hour are kept, so requests and caches still
//! pointing at a just-replaced object are served.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use regex::Regex;

use crate::Server;
use crate::snapshot::snapshot_key;

/// The files of one object generation: every encoding of one kind and hash.
struct Generation {
    kind: String,
    hash: String,
    newest: SystemTime,
    paths: Vec<PathBuf>,
}

impl Server {
    /// Deletes a term's object generations that are neither current, nor the
    /// newest previous one of their kind, nor modified within the last hour.
    /// Called with the snapshot lock held after the manifest commit.
    ///
    /// Keeping a previous generation and an hour of overlap serves requests
    /// and caches still pointing at it, rather than accumulating seat
    /// objects forever.
    pub(crate) async fn cleanup_snapshot_versions(&self, inst: &str, term: &str) -> anyhow::Result<()> {
        let dir = self.data_dir.join("catalog").join(inst);
        let pattern = Regex::new(&format!(
            r"^{}\.(structure|seats|desc)\.([0-9a-f]{{64}})\.json(?:\.(?:gz|br))?$",
            regex::escape(term)
        ))?;
        let mut groups: HashMap<String, Generation> = HashMap::new();
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(found) = pattern.captures(name) else { continue };
            let metadata = entry.metadata().await?;
            if metadata.is_dir() {
                continue;
            }
            let modified = metadata.modified()?;
            let (kind, hash) = (&found[1], &found[2]);
            let group = groups.entry(format!("{kind}:{hash}")).or_insert_with(|| Generation {
                kind: kind.to_owned(),
                hash: hash.to_owned(),
                newest: SystemTime::UNIX_EPOCH,
                paths: Vec::new(),
            });
            group.paths.push(dir.join(name));
            if modified > group.newest {
                group.newest = modified;
            }
        }
        let mut current = HashMap::new();
        for kind in ["structure", "seats", "desc"] {
            current.insert(kind.to_owned(), self.kv_get(&snapshot_key(kind, inst, term)).await);
        }
        let is_current = |g: &Generation| current.get(&g.kind).is_some_and(|h| *h == g.hash);
        let mut previous: HashMap<&str, &Generation> = HashMap::new();
        for g in groups.values() {
            if is_current(g) {
                continue;
            }
            let newer = match previous.get(g.kind.as_str()) {
                None => true,
                Some(p) => g.newest > p.newest || (g.newest == p.newest && g.hash > p.hash),
            };
            if newer {
                previous.insert(&g.kind, g);
            }
        }
        let cutoff: SystemTime = (self.now() - chrono::TimeDelta::hours(1)).into();
        for g in groups.values() {
            let kept = previous.get(g.kind.as_str()).is_some_and(|p| std::ptr::eq(*p, g));
            if is_current(g) || kept || g.newest >= cutoff {
                continue;
            }
            for path in &g.paths {
                match tokio::fs::remove_file(path).await {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(anyhow::anyhow!("remove expired snapshot: {err}")),
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone, Utc};

    use crate::snapshot::{exists, snapshot_file, snapshot_key, with_suffix};
    use crate::test_support::test_server;

    #[tokio::test]
    async fn snapshot_cleanup_keeps_current_previous_and_recent_objects() {
        let t = test_server().await;
        let s = &t.server;
        let dir = s.data_dir.join("catalog").join("demo");
        std::fs::create_dir_all(&dir).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 8, 12, 0, 0).unwrap();
        s.clock.set(now);
        let hash = |i: usize| char::from(b'a' + i as u8).to_string().repeat(64);
        let ages = [TimeDelta::hours(5), TimeDelta::hours(4), TimeDelta::hours(2), TimeDelta::minutes(10)];
        for kind in ["structure", "seats", "desc"] {
            for (i, age) in ages.iter().enumerate() {
                // Current need not be the most recently written file after a restoration.
                if i == 1 {
                    s.kv_set(&snapshot_key(kind, "demo", "t"), &hash(i)).await;
                }
                let path = snapshot_file(&dir, "t", kind, &hash(i));
                for suffix in ["", ".gz", ".br"] {
                    let file = with_suffix(&path, suffix);
                    std::fs::write(&file, b"test").unwrap();
                    let at: std::time::SystemTime = (now - *age).into();
                    std::fs::File::options().write(true).open(&file).unwrap().set_modified(at).unwrap();
                }
            }
        }
        s.cleanup_snapshot_versions("demo", "t").await.unwrap();
        for kind in ["structure", "seats", "desc"] {
            for i in 0..4 {
                let path = snapshot_file(&dir, "t", kind, &hash(i));
                for suffix in ["", ".gz", ".br"] {
                    let kept = exists(&with_suffix(&path, suffix));
                    assert_eq!(kept, i == 1 || i == 3, "generation {kind}/{i}{suffix} kept={kept}");
                }
            }
        }
        // The newest previous generation remains even after its grace period expires.
        s.clock.set(now + TimeDelta::hours(2));
        s.cleanup_snapshot_versions("demo", "t").await.unwrap();
        assert!(exists(&snapshot_file(&dir, "t", "seats", &hash(3))), "newest previous generation deleted");
    }
}
