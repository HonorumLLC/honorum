//! Catalog snapshots: each term is published as immutable,
//! content-addressed objects (structure, descriptions, seats) with gzip and
//! Brotli copies, and a `kv` manifest switched once every file is written.
//! Nothing is written when no input changed.
//!
//! The snapshot is what every visitor downloads for a term, so it carries
//! only what the planner renders. Per-section fields that repeat the course
//! (code, subject, title), the term (dates), or the institution (source URL)
//! are left out and filled back in by the client; descriptions and
//! prerequisites go to a sidecar object fetched only when a class is opened.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use honorum_catalog::{Exam, Meeting, Section, Term};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::Server;
use crate::api::null_default;
use crate::db::{Sql, Tx};

/// One section in a snapshot, without what repeats the course, term, or
/// school (filled back in by the client).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SnapSection {
    pub(crate) id: String,
    /// Only when it differs from the course title.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) title: String,
    #[serde(rename = "num")]
    pub(crate) number: String,
    #[serde(rename = "type", skip_serializing_if = "String::is_empty")]
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) link: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) instructors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) meetings: Vec<Meeting>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) exams: Vec<Exam>,
    #[serde(skip_serializing_if = "is_zero", serialize_with = "compact_float")]
    pub(crate) credits: f64,
    /// Only when they differ from the term dates.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) start: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) end: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) mode: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) attributes: Vec<String>,
    /// Only when it differs from the term's common source URL.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) url: String,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// Writes a float in its shortest JSON form, with whole numbers below 10^15
/// as integers (`3`, not `3.0`). NaN and infinities fail. Stored sections
/// and published snapshots were written this way, so changing it changes
/// every stored row and snapshot hash.
pub(crate) fn compact_float<S: serde::Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if !v.is_finite() {
        return Err(serde::ser::Error::custom(format!("json: unsupported value: {v}")));
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return s.serialize_i64(*v as i64);
    }
    s.serialize_f64(*v)
}

/// One course and its sections.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SnapCourse {
    pub(crate) code: String,
    pub(crate) subject: String,
    pub(crate) title: String,
    /// Names of the terms this course appears in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) offered: Vec<String>,
    pub(crate) sections: Vec<SnapSection>,
}

/// One course's entry in the descriptions object.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SnapDesc {
    #[serde(rename = "d", skip_serializing_if = "String::is_empty")]
    pub(crate) description: String,
    #[serde(rename = "p", skip_serializing_if = "String::is_empty")]
    pub(crate) prereqs: String,
}

/// The structure object (version 3). The published file adds `"hash"`, the
/// SHA-256 of this value's JSON.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SnapshotContent {
    #[serde(rename = "v")]
    pub(crate) version: i64,
    pub(crate) inst: String,
    pub(crate) term: Term,
    pub(crate) url: String,
    pub(crate) courses: Vec<SnapCourse>,
}

/// The published structure file: the content followed by its hash.
#[derive(Serialize)]
struct PublishedStructure<'a> {
    #[serde(flatten)]
    content: &'a SnapshotContent,
    hash: &'a str,
}

/// The seats object: section id → `[capacity, available, waitlist]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SeatSnapshot {
    #[serde(rename = "v")]
    pub(crate) version: i64,
    pub(crate) inst: String,
    pub(crate) term: String,
    pub(crate) seats: BTreeMap<String, [i64; 3]>,
}

/// A stored section: every field optional, `null` read as the default.
/// Stored rows omit seat counts.
#[derive(Deserialize, Default)]
#[serde(default)]
struct StoredSection {
    #[serde(deserialize_with = "null_default")]
    id: String,
    #[serde(deserialize_with = "null_default")]
    course: String,
    #[serde(deserialize_with = "null_default")]
    subject: String,
    #[serde(deserialize_with = "null_default")]
    title: String,
    #[serde(rename = "num", deserialize_with = "null_default")]
    number: String,
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: String,
    #[serde(deserialize_with = "null_default")]
    link: String,
    #[serde(deserialize_with = "null_default")]
    instructors: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    meetings: Vec<Meeting>,
    #[serde(deserialize_with = "null_default")]
    exams: Vec<Exam>,
    #[serde(deserialize_with = "null_default")]
    capacity: i64,
    #[serde(deserialize_with = "null_default")]
    available: i64,
    #[serde(deserialize_with = "null_default")]
    waitlist: i64,
    #[serde(deserialize_with = "null_default")]
    credits: f64,
    #[serde(deserialize_with = "null_default")]
    start: String,
    #[serde(deserialize_with = "null_default")]
    end: String,
    #[serde(deserialize_with = "null_default")]
    mode: String,
    #[serde(deserialize_with = "null_default")]
    attributes: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    description: String,
    #[serde(deserialize_with = "null_default")]
    prereqs: String,
    #[serde(rename = "url", deserialize_with = "null_default")]
    source_url: String,
}

/// Parses a stored section row (`sections.data`). Missing fields, including
/// the seat counts that live in `section_seats`, take their zero values.
pub(crate) fn parse_section(data: &str) -> serde_json::Result<Section> {
    let s: StoredSection = serde_json::from_str(data)?;
    Ok(Section {
        id: s.id,
        course: s.course,
        subject: s.subject,
        title: s.title,
        number: s.number,
        kind: s.kind,
        link: s.link,
        instructors: s.instructors,
        meetings: s.meetings,
        exams: s.exams,
        capacity: s.capacity,
        available: s.available,
        waitlist: s.waitlist,
        credits: s.credits,
        start: s.start,
        end: s.end,
        mode: s.mode,
        attributes: s.attributes,
        description: s.description,
        prereqs: s.prereqs,
        source_url: s.source_url,
    })
}

/// The `kv` key of a manifest entry: `snap:KIND:INST:TERM`.
pub(crate) fn snapshot_key(kind: &str, inst: &str, term: &str) -> String {
    format!("snap:{kind}:{inst}:{term}")
}

/// Hex SHA-256 of an object body.
pub(crate) fn snapshot_digest(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// The path of one object: `DIR/TERM.KIND.HASH.json`.
pub(crate) fn snapshot_file(dir: &Path, term: &str, kind: &str, hash: &str) -> PathBuf {
    dir.join(format!("{term}.{kind}.{hash}.json"))
}

/// Reports whether an object and its `.gz` and `.br` copies all exist.
pub(crate) fn complete_snapshot(path: &Path) -> bool {
    exists(path) && exists(&with_suffix(path, ".gz")) && exists(&with_suffix(path, ".br"))
}

/// `path` with `suffix` appended to its file name.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Reports whether a file exists.
pub(crate) fn exists(path: &Path) -> bool {
    path.exists()
}

/// Maps each course code to the names of the terms it appears in, in term
/// id order.
async fn offered_terms(tx: &Tx<'_>, inst: &str) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let rows = tx
        .fetch_all(
            "SELECT DISTINCT sections.term, terms.name, json_text(sections.data,'$.course')
		FROM sections JOIN terms ON terms.inst=sections.inst AND terms.id=sections.term
		WHERE sections.inst=$1 ORDER BY sections.term",
            &[&inst],
        )
        .await?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let name: String = row.try_get(1)?;
        let code: String = row.try_get(2)?;
        out.entry(code).or_default().push(name);
    }
    Ok(out)
}

/// Every structural input of a school's snapshots, including other terms'
/// offerings and names, as JSON. Freshness-only and seat-only writes do not
/// change it.
async fn snapshot_dependencies(tx: &Tx<'_>, inst: &str) -> anyhow::Result<String> {
    let rows = tx
        .fetch_all(
            r#"SELECT t.id,t.name,COALESCE(t.start,''),COALESCE(t."end",''),COALESCE(r.structure,0) FROM terms t LEFT JOIN catalog_revisions r ON r.inst=t.inst AND r.term=t.id WHERE t.inst=$1 ORDER BY t.id"#,
            &[&inst],
        )
        .await?;
    let mut deps = Vec::with_capacity(rows.len());
    for row in rows {
        let (id, name, start, end): (String, String, String, String) =
            (row.try_get(0)?, row.try_get(1)?, row.try_get(2)?, row.try_get(3)?);
        let revision: i64 = row.try_get(4)?;
        deps.push(serde_json::json!([id, name, start, end, revision]));
    }
    // No terms is recorded as `null`, the value stored by earlier releases,
    // so an unchanged school is not republished.
    if deps.is_empty() {
        return Ok("null".into());
    }
    Ok(serde_json::to_string(&deps)?)
}

async fn read_kv(tx: &Tx<'_>, key: &str) -> anyhow::Result<String> {
    let row = tx.fetch_opt("SELECT v FROM kv WHERE k=$1", &[&key]).await?;
    Ok(match row {
        Some(row) => row.try_get::<_, Option<String>>(0)?.unwrap_or_default(),
        None => String::new(),
    })
}

async fn read_snapshot_seats(tx: &Tx<'_>, inst: &str, term: &str) -> anyhow::Result<(SeatSnapshot, i64)> {
    let mut out = SeatSnapshot { version: 1, inst: inst.to_owned(), term: term.to_owned(), seats: BTreeMap::new() };
    let revision: i64 = tx
        .fetch_one(
            "SELECT COALESCE((SELECT seats FROM catalog_revisions WHERE inst=$1 AND term=$2),0)",
            &[&inst, &term],
        )
        .await?
        .try_get(0)?;
    let rows = tx
        .fetch_all(
            "SELECT id,capacity,available,waitlist FROM section_seats WHERE inst=$1 AND term=$2",
            &[&inst, &term],
        )
        .await?;
    for row in rows {
        out.seats.insert(row.try_get(0)?, [row.try_get(1)?, row.try_get(2)?, row.try_get(3)?]);
    }
    Ok((out, revision))
}

/// Groups a term's sections into courses and splits their descriptions into
/// a separate object.
async fn build_structure(tx: &Tx<'_>, inst: &str, t: &Term) -> anyhow::Result<(SnapshotContent, Vec<u8>)> {
    let rows = tx.fetch_all("SELECT data FROM sections WHERE inst=$1 AND term=$2 ORDER BY id", &[&inst, &t.id]).await?;
    let mut secs = Vec::with_capacity(rows.len());
    let mut urls: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let data: String = row.try_get(0)?;
        let sec =
            parse_section(&data).map_err(|err| anyhow::anyhow!("snapshot {inst}/{}: invalid section: {err}", t.id))?;
        *urls.entry(sec.source_url.clone()).or_default() += 1;
        secs.push(sec);
    }
    let offered = offered_terms(tx, inst).await?;
    let mut common_url = String::new();
    for (u, &n) in &urls {
        let best = urls.get(&common_url).copied().unwrap_or(0);
        if n > best || (n == best && *u > common_url) {
            common_url = u.clone();
        }
    }
    let titles = course_titles(&secs);
    let mut by_code: HashMap<String, SnapCourse> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut descs: BTreeMap<String, SnapDesc> = BTreeMap::new();
    for sec in secs {
        let course = by_code.entry(sec.course.clone()).or_insert_with(|| {
            order.push(sec.course.clone());
            SnapCourse {
                code: sec.course.clone(),
                subject: sec.subject.clone(),
                title: titles.get(&sec.course).cloned().unwrap_or_default(),
                offered: offered.get(&sec.course).cloned().unwrap_or_default(),
                sections: Vec::new(),
            }
        });
        let d = descs.get(&sec.course);
        let empty = d.is_none_or(|d| d.description.is_empty() && d.prereqs.is_empty());
        if empty && (!sec.description.is_empty() || !sec.prereqs.is_empty()) {
            descs.insert(
                sec.course.clone(),
                SnapDesc { description: sec.description.clone(), prereqs: sec.prereqs.clone() },
            );
        }
        let mut ss = SnapSection {
            id: sec.id,
            number: sec.number,
            kind: sec.kind,
            link: sec.link,
            instructors: sec.instructors,
            meetings: sec.meetings,
            exams: sec.exams,
            credits: sec.credits,
            mode: sec.mode,
            attributes: sec.attributes,
            ..SnapSection::default()
        };
        if sec.title != course.title {
            ss.title = sec.title;
        }
        if sec.start != t.start {
            ss.start = sec.start;
        }
        if sec.end != t.end {
            ss.end = sec.end;
        }
        if sec.source_url != common_url {
            ss.url = sec.source_url;
        }
        course.sections.push(ss);
    }
    let courses = order.iter().filter_map(|code| by_code.remove(code)).collect();
    let desc_body = serde_json::to_vec(&descs)?;
    Ok((SnapshotContent { version: 3, inst: inst.to_owned(), term: t.clone(), url: common_url, courses }, desc_body))
}

impl Server {
    /// Writes manifest values in one transaction and invalidates the public
    /// configuration.
    async fn publish_snapshot_manifest(&self, values: &[(String, String)]) -> anyhow::Result<()> {
        let mut client = self.db.client().await?;
        let tx = client.transaction().await?;
        for (key, value) in values {
            tx.exec("INSERT INTO kv(k,v) VALUES($1,$2) ON CONFLICT(k) DO UPDATE SET v=excluded.v", &[key, value])
                .await?;
        }
        tx.commit().await?;
        self.invalidate_public_config();
        Ok(())
    }

    /// Publishes a term's structure, description, and seat objects under the
    /// `publication` advisory lock and the snapshot lock (90-second budget),
    /// then switches their manifest together, queues reminder rebuilds when
    /// the structure changed, and removes expired versions. Reuses the
    /// structure on seat-only changes; skips everything when nothing changed.
    pub(crate) async fn write_snapshot(&self, inst: &str, term: &str) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(90), self.write_snapshot_now(inst, term))
            .await
            .map_err(|_| anyhow::anyhow!("context deadline exceeded"))?
    }

    async fn write_snapshot_now(&self, inst: &str, term: &str) -> anyhow::Result<()> {
        let lease = self.db.lock("publication", &format!("{inst}:{term}")).await?;
        let lost = lease.lost();
        tokio::select! {
            result = self.publish_term(inst, term) => result,
            () = lost.cancelled() => Err(anyhow::anyhow!("publication lock lost")),
        }
    }

    async fn publish_term(&self, inst: &str, term: &str) -> anyhow::Result<()> {
        let _guard = self.snapshot_lock.lock().await;
        let mut client = self.db.client().await?;
        let tx = client.build_transaction().read_only(true).start().await?;
        let row = tx
            .fetch_one(
                r#"SELECT id,name,COALESCE(start,''),COALESCE("end",'') FROM terms WHERE inst=$1 AND id=$2"#,
                &[&inst, &term],
            )
            .await?;
        let t = Term { id: row.try_get(0)?, name: row.try_get(1)?, start: row.try_get(2)?, end: row.try_get(3)? };
        let deps = snapshot_dependencies(&tx, inst).await?;
        let seat_rev: i64 = tx
            .fetch_one(
                "SELECT COALESCE((SELECT seats FROM catalog_revisions WHERE inst=$1 AND term=$2),0)",
                &[&inst, &term],
            )
            .await?
            .try_get(0)?;
        let key = |kind: &str| snapshot_key(kind, inst, term);
        let mut structure_hash = read_kv(&tx, &key("structure")).await?;
        let mut desc_hash = read_kv(&tx, &key("desc")).await?;
        let seats_hash = read_kv(&tx, &key("seats")).await?;
        let inputs = read_kv(&tx, &key("inputs")).await?;
        let structure_inputs = read_kv(&tx, &key("structure-inputs")).await?;
        let dir = self.data_dir.join("catalog").join(inst);
        // Structure and descriptions are reused on seat-only changes. This
        // avoids SQL JSON reads, offered-term aggregation and grouping on the
        // frequent availability path.
        let reused = structure_inputs == deps
            && !structure_hash.is_empty()
            && !desc_hash.is_empty()
            && complete_snapshot(&snapshot_file(&dir, term, "structure", &structure_hash))
            && complete_snapshot(&snapshot_file(&dir, term, "desc", &desc_hash));
        if reused
            && inputs == format!("{deps}|{seat_rev}")
            && !seats_hash.is_empty()
            && complete_snapshot(&snapshot_file(&dir, term, "seats", &seats_hash))
        {
            tx.commit().await?;
            for (kind, hash) in [("structure", &structure_hash), ("desc", &desc_hash), ("seats", &seats_hash)] {
                self.upload_snapshot_files(inst, term, kind, hash).await?;
            }
            self.counters.snapshot_skips.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let mut objects: Vec<(&str, String, Vec<u8>)> = Vec::new();
        if !reused {
            let (content, desc_body) = build_structure(&tx, inst, &t).await?;
            let canonical = serde_json::to_vec(&content)?;
            structure_hash = snapshot_digest(&canonical);
            desc_hash = snapshot_digest(&desc_body);
            let structure_body = serde_json::to_vec(&PublishedStructure { content: &content, hash: &structure_hash })?;
            objects.push(("structure", structure_hash.clone(), structure_body));
            objects.push(("desc", desc_hash.clone(), desc_body));
        }
        let (seats, seat_rev) = read_snapshot_seats(&tx, inst, term).await?;
        tx.commit().await?;
        drop(client);
        let seats_body = serde_json::to_vec(&seats)?;
        let seats_hash = snapshot_digest(&seats_body);
        objects.push(("seats", seats_hash.clone(), seats_body));
        write_objects(&dir, term, objects).await?;
        for (kind, hash) in [("structure", &structure_hash), ("desc", &desc_hash), ("seats", &seats_hash)] {
            self.upload_snapshot_files(inst, term, kind, hash).await?;
        }
        let values = vec![
            (key("structure"), structure_hash),
            (key("desc"), desc_hash),
            (key("seats"), seats_hash),
            (key("inputs"), format!("{deps}|{seat_rev}")),
            (key("structure-inputs"), deps),
            (key("seat-revision"), seat_rev.to_string()),
        ];
        self.publish_snapshot_manifest(&values).await?;
        if !reused {
            self.exec_logged(
                "INSERT INTO work_jobs(kind,key,account,run_at) SELECT 'reminder','acct:'||account,account,0 FROM docs WHERE id='prefs' AND deleted=0 AND json_text(body,'$.inst')=$1 ON CONFLICT(kind,key) DO UPDATE SET generation=work_jobs.generation+1,run_at=0",
                &[&inst],
            )
            .await;
        }
        self.counters.snapshot_publications.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = self.cleanup_snapshot_versions(inst, term).await {
            tracing::warn!(%err, "catalog version cleanup");
        }
        Ok(())
    }

    /// Publishes only the seat object, cheaply skipped when the seat
    /// revision has not changed; does nothing for a term with no revision.
    pub(crate) async fn write_seat_snapshot(&self, inst: &str, term: &str) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(90), async {
            let lease = self.db.lock("publication", &format!("{inst}:{term}")).await?;
            let lost = lease.lost();
            tokio::select! {
                result = self.publish_seats(inst, term) => result,
                () = lost.cancelled() => Err(anyhow::anyhow!("publication lock lost")),
            }
        })
        .await
        .map_err(|_| anyhow::anyhow!("context deadline exceeded"))?
    }

    async fn publish_seats(&self, inst: &str, term: &str) -> anyhow::Result<()> {
        let _guard = self.snapshot_lock.lock().await;
        let mut client = self.db.client().await?;
        let tx = client.build_transaction().read_only(true).start().await?;
        let Some(row) =
            tx.fetch_opt("SELECT seats FROM catalog_revisions WHERE inst=$1 AND term=$2", &[&inst, &term]).await?
        else {
            return Ok(());
        };
        let revision: i64 = row.try_get(0)?;
        let hash = read_kv(&tx, &snapshot_key("seats", inst, term)).await?;
        let prior = read_kv(&tx, &snapshot_key("seat-revision", inst, term)).await?;
        let dir = self.data_dir.join("catalog").join(inst);
        if !hash.is_empty()
            && prior == revision.to_string()
            && complete_snapshot(&snapshot_file(&dir, term, "seats", &hash))
        {
            tx.commit().await?;
            self.upload_snapshot_files(inst, term, "seats", &hash).await?;
            self.counters.snapshot_skips.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let (seats, revision) = read_snapshot_seats(&tx, inst, term).await?;
        tx.commit().await?;
        drop(client);
        let body = serde_json::to_vec(&seats)?;
        let hash = snapshot_digest(&body);
        write_objects(&dir, term, vec![("seats", hash.clone(), body)]).await?;
        self.upload_snapshot_files(inst, term, "seats", &hash).await?;
        self.publish_snapshot_manifest(&[
            (snapshot_key("seats", inst, term), hash),
            (snapshot_key("seat-revision", inst, term), revision.to_string()),
        ])
        .await?;
        self.counters.snapshot_publications.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = self.cleanup_snapshot_versions(inst, term).await {
            tracing::warn!(%err, "catalog version cleanup");
        }
        Ok(())
    }

    /// Rebuilds the files of every collected term of a configured school
    /// from the database, so a fresh disk serves catalogs at once. Returns
    /// the joined failures; stops early when `stop` fires.
    pub async fn restore_snapshots(&self, stop: &CancellationToken) -> anyhow::Result<()> {
        let rows = self.db.fetch_all("SELECT inst, id FROM terms WHERE updated_at>0", &[]).await?;
        let mut pairs = Vec::with_capacity(rows.len());
        for row in rows {
            pairs.push((row.try_get::<_, String>(0)?, row.try_get::<_, String>(1)?));
        }
        let mut n = 0;
        let mut failures = Vec::new();
        for (inst, term) in pairs {
            if stop.is_cancelled() {
                anyhow::bail!("context canceled");
            }
            if self.inst(&inst).is_none() {
                continue;
            }
            let result = tokio::select! {
                result = self.write_snapshot(&inst, &term) => result,
                () = stop.cancelled() => anyhow::bail!("context canceled"),
            };
            match result {
                Ok(()) => n += 1,
                Err(err) => failures.push(format!("{err:#}")),
            }
        }
        if !failures.is_empty() {
            anyhow::bail!(failures.join("\n"));
        }
        tracing::info!(terms = n, "catalog snapshots ready");
        Ok(())
    }
}

/// Writes each object that is not already complete, off the async runtime.
async fn write_objects(dir: &Path, term: &str, objects: Vec<(&str, String, Vec<u8>)>) -> anyhow::Result<()> {
    let dir = dir.to_owned();
    let files: Vec<(PathBuf, Vec<u8>)> =
        objects.into_iter().map(|(kind, hash, body)| (snapshot_file(&dir, term, kind, &hash), body)).collect();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        for (path, body) in files {
            if !complete_snapshot(&path) {
                write_gzipped(&path, &body)?;
            }
        }
        Ok(())
    })
    .await
    .context("snapshot writer")??;
    Ok(())
}

/// Writes an object and its gzip and Brotli (both level 6) copies through
/// temporary files, renaming the compressed copies first, so a complete
/// object never lacks an encoding.
pub(crate) fn write_gzipped(path: &Path, body: &[u8]) -> std::io::Result<()> {
    // Level 6 publishes several times faster than the maximum for a few
    // percent more bytes; publication happens on the request path after a
    // container restart, so speed matters more than the last kilobytes.
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    gz.write_all(body)?;
    let gz = gz.finish()?;
    let mut br = Vec::new();
    {
        // A window no larger than the object (from 1 KiB up to Brotli's
        // 4 MiB default) compresses it as well with far less memory.
        let window = (usize::BITS - body.len().saturating_sub(1).leading_zeros()).clamp(10, 22);
        let mut writer = brotli::CompressorWriter::new(&mut br, 4096, 6, window);
        writer.write_all(body)?;
        writer.flush()?;
    }
    for (suffix, data) in [("", body), (".gz", &gz[..]), (".br", &br[..])] {
        std::fs::write(with_suffix(path, &format!("{suffix}.tmp")), data)?;
    }
    for suffix in [".gz", ".br", ""] {
        std::fs::rename(with_suffix(path, &format!("{suffix}.tmp")), with_suffix(path, suffix))?;
    }
    Ok(())
}

/// A section "title" that only names what the section is attached to, like
/// ClassNav's "Disc-PHIL 1273-030" or "Lab-CHEM 1315-001".
static LINK_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(disc|discussion|lab|laboratory|rec|recitation|lecture|lec)(?-u:\b)[\s-]*[A-Z]")
        .expect("valid pattern")
});

/// One title per course: the most common among its lecture sections, else
/// among all sections, preferring real titles to link labels such as
/// "Lab-CHEM 1315-001"; ties go to the earliest section.
///
/// Some schools title each section separately (honors, topics), and
/// discussion rows sometimes carry no title of their own.
pub(crate) fn course_titles(secs: &[Section]) -> HashMap<String, String> {
    #[derive(Default)]
    struct Tally {
        lec: HashMap<String, i64>,
        all: HashMap<String, i64>,
        /// Titles in order of first appearance, the tie-breaker.
        seen: Vec<String>,
        first: String,
    }
    let mut by_code: HashMap<&str, Tally> = HashMap::new();
    for sec in secs {
        let t = by_code.entry(&sec.course).or_insert_with(|| Tally { first: sec.title.clone(), ..Tally::default() });
        if sec.title.is_empty() {
            continue;
        }
        if !t.all.contains_key(&sec.title) {
            t.seen.push(sec.title.clone());
        }
        *t.all.entry(sec.title.clone()).or_default() += 1;
        if sec.kind == "LEC" {
            *t.lec.entry(sec.title.clone()).or_default() += 1;
        }
    }
    fn best(m: &HashMap<String, i64>, seen: &[String]) -> String {
        let mut top = String::new();
        for title in seen {
            let Some(&n) = m.get(title) else { continue };
            if LINK_LABEL.is_match(title) && m.len() > 1 {
                continue;
            }
            if top.is_empty() || n > m.get(&top).copied().unwrap_or(0) {
                top = title.clone();
            }
        }
        top
    }
    by_code
        .into_iter()
        .map(|(code, t)| {
            let mut title = best(&t.lec, &t.seen);
            if title.is_empty() {
                title = best(&t.all, &t.seen);
            }
            if title.is_empty() {
                title = t.first;
            }
            (code.to_owned(), title)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::monitor::tests::published;
    use crate::test_support::test_server;

    /// The published structure file as clients read it.
    #[derive(Deserialize)]
    struct Published {
        #[serde(flatten)]
        content: SnapshotContent,
        hash: String,
    }

    #[tokio::test]
    async fn snapshot_updates_inherited_metadata() {
        for field in ["dates", "source URL"] {
            let t = test_server().await;
            let s = &t.server;
            let mut term =
                Term { id: "test".into(), name: "Fall".into(), start: "2026-08-24".into(), end: "2026-12-18".into() };
            let mut sec = Section {
                id: "one".into(),
                course: "CS 101".into(),
                title: "Intro".into(),
                start: term.start.clone(),
                end: term.end.clone(),
                source_url: "https://school.example/old".into(),
                ..Section::default()
            };
            let write = |term: Term, sec: Section| async move {
                s.store_catalog(&s.insts[0], &term, &[sec]).await.unwrap();
                s.write_snapshot("demo", &term.id).await.unwrap();
                let (path, _) = published(s, "structure", "demo", &term.id).await;
                std::fs::read(path).unwrap()
            };
            let before = write(term.clone(), sec.clone()).await;
            assert_eq!(before, write(term.clone(), sec.clone()).await, "{field}: unchanged catalog was rewritten");
            if field == "dates" {
                term.start = "2026-08-31".into();
                sec.start = "2026-08-31".into();
            } else {
                sec.source_url = "https://school.example/new".into();
            }
            let after = write(term.clone(), sec.clone()).await;
            let old: Published = serde_json::from_slice(&before).unwrap();
            let got: Published = serde_json::from_slice(&after).unwrap();
            assert!(
                old.hash != got.hash && got.content.term.start == term.start && got.content.url == sec.source_url,
                "snapshot retained stale {field}: {:?}",
                got.content
            );
        }
    }

    #[tokio::test]
    async fn snapshot_keeps_published_catalog_on_invalid_data() {
        let t = test_server().await;
        let s = &t.server;
        s.sync_catalog(&s.insts[0], None, true).await.unwrap();
        let (path, _) = published(s, "structure", "demo", "2026FA").await;
        let before = std::fs::read(&path).unwrap();
        s.db.exec("UPDATE sections SET data='invalid json' WHERE inst='demo' AND term='2026FA'", &[]).await.unwrap();
        assert!(s.write_snapshot("demo", "2026FA").await.is_err(), "snapshot accepted invalid section data");
        assert_eq!(std::fs::read(&path).unwrap(), before, "failed snapshot replaced the published catalog");
        assert!(s.write_snapshot("demo", "missing").await.is_err(), "snapshot accepted a missing term");
    }

    #[test]
    fn course_titles_prefer_lectures() {
        let sec = |course: &str, number: &str, kind: &str, title: &str| Section {
            course: course.into(),
            number: number.into(),
            kind: kind.into(),
            title: title.into(),
            ..Section::default()
        };
        let secs = [
            sec("PHIL 1273", "039", "DIS", "Disc-PHIL 1273-030"),
            sec("PHIL 1273", "030", "LEC", "Intro to Business Ethics"),
            sec("PHIL 1273", "038", "DIS", "Disc-PHIL 1273-030"),
            sec("PHIL 1273", "015", "LEC", "Intro Business Ethics (HONORS)"),
            sec("CHEM 1315", "001", "LAB", "Lab-CHEM 1315-001"),
            sec("CHEM 1315", "002", "LAB", "General Chemistry"),
            sec("MATH 1914", "001", "DIS", "Disc-MATH 1914-001"),
            sec("ENGL 1113", "001", "LEC", ""),
            sec("ENGL 1113", "002", "LEC", "Principles of English Composition"),
        ];
        let got = course_titles(&secs);
        for (code, want) in [
            // Lectures win; the discussion label is never the course title.
            ("PHIL 1273", "Intro to Business Ethics"),
            // A link label loses to any real title.
            ("CHEM 1315", "General Chemistry"),
            // Nothing better exists.
            ("MATH 1914", "Disc-MATH 1914-001"),
            // Empty titles are ignored.
            ("ENGL 1113", "Principles of English Composition"),
        ] {
            assert_eq!(got.get(code).map(String::as_str), Some(want), "{code}");
        }
    }

    #[tokio::test]
    async fn independent_snapshot_versions_and_restoration() {
        let t = test_server().await;
        let s = &t.server;
        let term = Term { id: "t".into(), name: "Fall".into(), start: "2026-08-01".into(), end: "2026-12-01".into() };
        let mut secs = vec![Section {
            id: "s".into(),
            course: "CS 101".into(),
            subject: "CS".into(),
            title: "Intro".into(),
            available: 0,
            capacity: 20,
            description: "Original description".into(),
            ..Section::default()
        }];
        let publish = |secs: Vec<Section>| {
            let term = term.clone();
            async move {
                s.store_catalog(&s.insts[0], &term, &secs).await.unwrap();
                s.write_snapshot("demo", &term.id).await.unwrap();
            }
        };
        let hash = |kind: &'static str| async move { s.kv_get(&snapshot_key(kind, "demo", "t")).await };
        publish(secs.clone()).await;
        let (structure, description, seats) = (hash("structure").await, hash("desc").await, hash("seats").await);
        let dir = s.data_dir.join("catalog").join("demo");
        let structure_path = snapshot_file(&dir, "t", "structure", &structure);
        let original = std::fs::read(&structure_path).unwrap();
        let text = String::from_utf8_lossy(&original);
        assert!(
            !text.contains(r#""available""#) && !text.contains(r#""capacity""#) && !text.contains(r#""waitlist""#),
            "mutable seat fields in structural snapshot"
        );
        let old_seat_path = snapshot_file(&dir, "t", "seats", &seats);
        let old_seat_body = std::fs::read(&old_seat_path).unwrap();
        s.store_seats(
            &s.insts[0],
            "t",
            &[honorum_catalog::Seats { id: "s".into(), capacity: 20, available: 2, waitlist: 0 }],
        )
        .await
        .unwrap();
        assert!(
            hash("structure").await == structure && hash("desc").await == description && hash("seats").await != seats,
            "seat poll invalidated unrelated snapshots"
        );
        assert_eq!(std::fs::read(&old_seat_path).unwrap(), old_seat_body, "old immutable seat object overwritten");
        secs[0].available = 2;
        publish(secs.clone()).await;
        assert!(hash("structure").await == structure && hash("desc").await == description, "structural reuse failed");
        let new_seats = hash("seats").await;
        secs[0].description = "Edited description".into();
        publish(secs.clone()).await;
        assert!(
            hash("structure").await == structure
                && hash("seats").await == new_seats
                && hash("desc").await != description,
            "description edit invalidated unrelated snapshots"
        );
        let skips = s.counters.snapshot_skips.load(Ordering::Relaxed);
        publish(secs.clone()).await;
        assert!(s.counters.snapshot_skips.load(Ordering::Relaxed) > skips, "unchanged snapshot not skipped");
        std::fs::remove_dir_all(s.data_dir.join("catalog")).unwrap();
        s.restore_snapshots(&CancellationToken::new()).await.unwrap();
        assert_eq!(std::fs::read(&structure_path).unwrap(), original, "restoration changed immutable structure bytes");
    }

    #[tokio::test]
    async fn snapshot_tracks_other_term_offerings_and_metadata() {
        let t = test_server().await;
        let s = &t.server;
        let first = Term { id: "one".into(), name: "Fall".into(), ..Term::default() };
        let second = Term { id: "two".into(), name: "Spring".into(), ..Term::default() };
        let secs = [Section {
            id: "s".into(),
            course: "CS 101".into(),
            title: "Intro".into(),
            available: -1,
            ..Section::default()
        }];
        let current = || async { s.kv_get(&snapshot_key("structure", "demo", "one")).await };
        s.store_catalog(&s.insts[0], &first, &secs).await.unwrap();
        s.write_snapshot("demo", "one").await.unwrap();
        let before = current().await;
        s.store_catalog(&s.insts[0], &second, &secs).await.unwrap();
        s.write_snapshot("demo", "one").await.unwrap();
        let offered = current().await;
        assert_ne!(before, offered, "other term offerings missed");
        s.db.exec("UPDATE terms SET name='Spring renamed' WHERE inst='demo' AND id='two'", &[]).await.unwrap();
        s.write_snapshot("demo", "one").await.unwrap();
        assert_ne!(offered, current().await, "other term name change missed");
    }

    #[test]
    fn published_encodings_round_trip() {
        let dir = std::env::temp_dir().join(format!("honorum-encoding-{}", crate::session::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("snapshot.json");
        let body = r#"{"course":"CS 101","title":"Café and 日本語","available":0}"#.repeat(100).into_bytes();
        write_gzipped(&path, &body).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body, "raw changed");
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(with_suffix(&path, ".gz")).unwrap())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, body, "gzip");
        let mut decoded = Vec::new();
        brotli::Decompressor::new(std::fs::File::open(with_suffix(&path, ".br")).unwrap(), 4096)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, body, "Brotli");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
