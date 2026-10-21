//! The database schema.
//!
//! The schema is the `CREATE ... IF NOT EXISTS` statements below, applied at
//! every start under a transaction-scoped advisory lock so concurrent API and
//! worker starts do not race. A change that alters an existing table or
//! rewrites rows needs a migration here; remove it once every deployment has
//! run it, and record the last release that carries it in
//! `docs/deployment.md`.
//!
//! Account tables live in `public` and catalog tables in `catalog`. Both
//! schemas share a database so catalog updates and the seat alerts they
//! queue commit together.

use crate::db::{Db, Error, Result};

/// Account, sync, notification, and job tables.
const ACCOUNTS: &str = r#"
CREATE TABLE IF NOT EXISTS account_activity(account TEXT COLLATE "C", day TEXT COLLATE "C", PRIMARY KEY(account,day));
CREATE TABLE IF NOT EXISTS account_timezones(account TEXT COLLATE "C" PRIMARY KEY, timezone TEXT COLLATE "C" NOT NULL);
CREATE TABLE IF NOT EXISTS account_streaks(account TEXT COLLATE "C" PRIMARY KEY, start TEXT COLLATE "C" NOT NULL);
CREATE TABLE IF NOT EXISTS account_streak_days(account TEXT COLLATE "C", day TEXT COLLATE "C", clean BIGINT NOT NULL, PRIMARY KEY(account,day));
CREATE TABLE IF NOT EXISTS account_streak_rollups(account TEXT COLLATE "C" PRIMARY KEY, through_day TEXT COLLATE "C" NOT NULL, run BIGINT NOT NULL, longest BIGINT NOT NULL, last_clean TEXT COLLATE "C" NOT NULL);
CREATE TABLE IF NOT EXISTS account_streak_cache(account TEXT COLLATE "C" PRIMARY KEY, seq BIGINT NOT NULL, valid_from BIGINT NOT NULL, valid_until BIGINT NOT NULL, timezone TEXT COLLATE "C" NOT NULL, result TEXT COLLATE "C" NOT NULL);
CREATE TABLE IF NOT EXISTS canvas_connections(account TEXT COLLATE "C" PRIMARY KEY, id TEXT COLLATE "C" NOT NULL, base TEXT COLLATE "C" NOT NULL, credential BYTEA NOT NULL, synced BIGINT NOT NULL DEFAULT 0, error TEXT COLLATE "C" NOT NULL DEFAULT '', attempted BIGINT NOT NULL DEFAULT 0, modules BIGINT NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS canvas_items(account TEXT COLLATE "C", connection TEXT COLLATE "C", uid TEXT COLLATE "C", document TEXT COLLATE "C" NOT NULL, PRIMARY KEY(account,connection,uid));
CREATE TABLE IF NOT EXISTS feed_connections(account TEXT COLLATE "C", id TEXT COLLATE "C", credential BYTEA NOT NULL, label TEXT COLLATE "C" NOT NULL DEFAULT '', tz TEXT COLLATE "C" NOT NULL DEFAULT 'UTC', synced BIGINT NOT NULL DEFAULT 0, attempted BIGINT NOT NULL DEFAULT 0, error TEXT COLLATE "C" NOT NULL DEFAULT '', created_at BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(account,id));
CREATE TABLE IF NOT EXISTS google_connections(account TEXT COLLATE "C" PRIMARY KEY, id TEXT COLLATE "C" NOT NULL, subject TEXT COLLATE "C" NOT NULL DEFAULT '', email TEXT COLLATE "C" NOT NULL DEFAULT '', credential BYTEA NOT NULL, scopes TEXT COLLATE "C" NOT NULL DEFAULT '', error TEXT COLLATE "C" NOT NULL DEFAULT '', created_at BIGINT NOT NULL DEFAULT 0, publish BIGINT NOT NULL DEFAULT 0, tasks BIGINT NOT NULL DEFAULT 1, calendar TEXT COLLATE "C" NOT NULL DEFAULT '', published_seq BIGINT NOT NULL DEFAULT -1, published_at BIGINT NOT NULL DEFAULT 0, publish_retry BIGINT NOT NULL DEFAULT 0, publish_error TEXT COLLATE "C" NOT NULL DEFAULT '', failures BIGINT NOT NULL DEFAULT 0, show BIGINT NOT NULL DEFAULT 0, listed_at BIGINT NOT NULL DEFAULT 0, version BIGINT NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS google_published(account TEXT COLLATE "C", key TEXT COLLATE "C", event TEXT COLLATE "C" NOT NULL, hash TEXT COLLATE "C" NOT NULL, etag TEXT COLLATE "C" NOT NULL DEFAULT '', PRIMARY KEY(account,key));
CREATE TABLE IF NOT EXISTS google_calendars(account TEXT COLLATE "C", id TEXT COLLATE "C", name TEXT COLLATE "C" NOT NULL DEFAULT '', color TEXT COLLATE "C" NOT NULL DEFAULT '', main BIGINT NOT NULL DEFAULT 0, selected BIGINT NOT NULL DEFAULT 0, events TEXT COLLATE "C" NOT NULL DEFAULT '[]', synced BIGINT NOT NULL DEFAULT 0, attempted BIGINT NOT NULL DEFAULT 0, error TEXT COLLATE "C" NOT NULL DEFAULT '', dirty BIGINT NOT NULL DEFAULT 1, channel TEXT COLLATE "C" NOT NULL DEFAULT '', resource TEXT COLLATE "C" NOT NULL DEFAULT '', channel_token TEXT COLLATE "C" NOT NULL DEFAULT '', channel_expires BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(account,id));
CREATE INDEX IF NOT EXISTS google_calendars_channel ON google_calendars(channel) WHERE channel<>'';
CREATE TABLE IF NOT EXISTS google_states(state TEXT COLLATE "C" PRIMARY KEY, account TEXT COLLATE "C" NOT NULL, session TEXT COLLATE "C" NOT NULL, verifier TEXT COLLATE "C" NOT NULL, want TEXT COLLATE "C" NOT NULL, back TEXT COLLATE "C" NOT NULL, created_at BIGINT NOT NULL, handoff TEXT COLLATE "C" NOT NULL DEFAULT '', credential BYTEA, scopes TEXT COLLATE "C" NOT NULL DEFAULT '', subject TEXT COLLATE "C" NOT NULL DEFAULT '', email TEXT COLLATE "C" NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS google_states_handoff ON google_states(handoff) WHERE handoff<>'';
CREATE TABLE IF NOT EXISTS watches(id TEXT COLLATE "C" PRIMARY KEY, inst TEXT COLLATE "C", term TEXT COLLATE "C", section TEXT COLLATE "C", channel TEXT COLLATE "C", target TEXT COLLATE "C", created_at BIGINT, last_alert BIGINT DEFAULT 0, account TEXT COLLATE "C" DEFAULT '', endpoint TEXT COLLATE "C" NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS watches_section ON watches(inst, term, section);
CREATE INDEX IF NOT EXISTS watches_account ON watches(account);
CREATE INDEX IF NOT EXISTS watches_endpoint ON watches(endpoint);
CREATE TABLE IF NOT EXISTS alerts(key TEXT COLLATE "C" PRIMARY KEY, detected_at BIGINT, delivered_at BIGINT DEFAULT 0, error TEXT COLLATE "C" DEFAULT '');
CREATE TABLE IF NOT EXISTS deliveries(
 id TEXT COLLATE "C" PRIMARY KEY, event_key TEXT COLLATE "C" NOT NULL, source TEXT COLLATE "C" NOT NULL, source_id TEXT COLLATE "C" NOT NULL,
 account TEXT COLLATE "C" NOT NULL DEFAULT '', device TEXT COLLATE "C" NOT NULL DEFAULT '', channel TEXT COLLATE "C" NOT NULL, target TEXT COLLATE "C" NOT NULL,
 payload TEXT COLLATE "C" NOT NULL, due_at BIGINT NOT NULL, expires_at BIGINT NOT NULL,
 attempts BIGINT NOT NULL DEFAULT 0, lease_until BIGINT NOT NULL DEFAULT 0,
 delivered_at BIGINT NOT NULL DEFAULT 0, error TEXT COLLATE "C" NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS deliveries_due ON deliveries(due_at,lease_until) WHERE delivered_at=0;
CREATE TABLE IF NOT EXISTS sync(id TEXT COLLATE "C" PRIMARY KEY, blob BYTEA, updated_at BIGINT);
CREATE INDEX IF NOT EXISTS sync_updated ON sync(updated_at);
CREATE TABLE IF NOT EXISTS kv(k TEXT COLLATE "C" PRIMARY KEY, v TEXT COLLATE "C");
CREATE TABLE IF NOT EXISTS reminders(id TEXT COLLATE "C" PRIMARY KEY, target TEXT COLLATE "C", tz TEXT COLLATE "C", items TEXT COLLATE "C", updated_at BIGINT, endpoint TEXT COLLATE "C" NOT NULL DEFAULT '');
CREATE INDEX IF NOT EXISTS reminders_endpoint ON reminders(endpoint);
CREATE TABLE IF NOT EXISTS reminder_log(key TEXT COLLATE "C" PRIMARY KEY, sent_at BIGINT);
CREATE TABLE IF NOT EXISTS accounts(id TEXT COLLATE "C" PRIMARY KEY, phone TEXT COLLATE "C" NOT NULL, created_at BIGINT, seq BIGINT DEFAULT 0);
CREATE UNIQUE INDEX IF NOT EXISTS accounts_phone ON accounts(phone);
CREATE TABLE IF NOT EXISTS sms_consents(id TEXT COLLATE "C" PRIMARY KEY, phone TEXT COLLATE "C" NOT NULL, consented_at BIGINT NOT NULL, version TEXT COLLATE "C" NOT NULL);
CREATE INDEX IF NOT EXISTS sms_consents_date ON sms_consents(consented_at);
CREATE TABLE IF NOT EXISTS sessions(id TEXT COLLATE "C" PRIMARY KEY, account TEXT COLLATE "C", token_hash TEXT COLLATE "C" UNIQUE, name TEXT COLLATE "C", created_at BIGINT, last_seen BIGINT, push TEXT COLLATE "C" DEFAULT '');
CREATE INDEX IF NOT EXISTS sessions_account ON sessions(account);
CREATE INDEX IF NOT EXISTS sessions_active ON sessions(account,last_seen);
CREATE INDEX IF NOT EXISTS canvas_due ON canvas_connections(synced,attempted);
CREATE INDEX IF NOT EXISTS feeds_due ON feed_connections(synced,attempted);
CREATE TABLE IF NOT EXISTS docs(account TEXT COLLATE "C", id TEXT COLLATE "C", type TEXT COLLATE "C", seq BIGINT, updated_at BIGINT, deleted BIGINT DEFAULT 0, body TEXT COLLATE "C", PRIMARY KEY(account,id));
CREATE INDEX IF NOT EXISTS docs_seq ON docs(account, seq);
CREATE TABLE IF NOT EXISTS doc_history(account TEXT COLLATE "C", id TEXT COLLATE "C", rev BIGINT, updated_at BIGINT, deleted BIGINT DEFAULT 0, body TEXT COLLATE "C", archived_at BIGINT NOT NULL, PRIMARY KEY(account,id,rev));
CREATE INDEX IF NOT EXISTS doc_history_age ON doc_history(archived_at);
CREATE TABLE IF NOT EXISTS daily_metrics(day TEXT COLLATE "C", name TEXT COLLATE "C", value BIGINT NOT NULL, PRIMARY KEY(day,name));
CREATE TABLE IF NOT EXISTS work_jobs(kind TEXT COLLATE "C", key TEXT COLLATE "C", account TEXT COLLATE "C" NOT NULL DEFAULT '', payload TEXT COLLATE "C" NOT NULL DEFAULT '', run_at BIGINT NOT NULL, lease_until BIGINT NOT NULL DEFAULT 0, owner TEXT COLLATE "C" NOT NULL DEFAULT '', attempts BIGINT NOT NULL DEFAULT 0, generation BIGINT NOT NULL DEFAULT 1, error TEXT COLLATE "C" NOT NULL DEFAULT '', PRIMARY KEY(kind,key));
CREATE INDEX IF NOT EXISTS work_jobs_due ON work_jobs(run_at,lease_until);
CREATE TABLE IF NOT EXISTS reminder_schedule(id TEXT COLLATE "C" PRIMARY KEY, account TEXT COLLATE "C" NOT NULL DEFAULT '', seq BIGINT NOT NULL DEFAULT 0, target TEXT COLLATE "C" NOT NULL DEFAULT '', tz TEXT COLLATE "C" NOT NULL, items TEXT COLLATE "C" NOT NULL, next_at BIGINT NOT NULL, version TEXT COLLATE "C" NOT NULL, built_at BIGINT NOT NULL);
CREATE INDEX IF NOT EXISTS reminder_schedule_due ON reminder_schedule(next_at);
CREATE TABLE IF NOT EXISTS rate_limits(key TEXT COLLATE "C" PRIMARY KEY, hits TEXT COLLATE "C" NOT NULL DEFAULT '[]', expires BIGINT NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS rate_limits_expiry ON rate_limits(expires);
CREATE TABLE IF NOT EXISTS provider_slots(host TEXT COLLATE "C", slot BIGINT, owner TEXT COLLATE "C" NOT NULL DEFAULT '', lease_until BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(host,slot));
"#;

/// Catalog tables. Sections hold structural JSON; seat counts change
/// independently in `section_seats`. Triggers count structural and seat
/// revisions per term, so publication can skip work no input requires.
const CATALOG: &str = r#"
CREATE TABLE IF NOT EXISTS catalog.terms(inst TEXT COLLATE "C", id TEXT COLLATE "C", name TEXT COLLATE "C", start TEXT COLLATE "C", "end" TEXT COLLATE "C", updated_at BIGINT, PRIMARY KEY(inst,id));
CREATE TABLE IF NOT EXISTS catalog.sections(inst TEXT COLLATE "C", term TEXT COLLATE "C", id TEXT COLLATE "C", data TEXT COLLATE "C", PRIMARY KEY(inst,term,id));
CREATE TABLE IF NOT EXISTS catalog.adapter_status(inst TEXT COLLATE "C" PRIMARY KEY, ok BIGINT DEFAULT 0, message TEXT COLLATE "C" DEFAULT '', last_ok BIGINT DEFAULT 0, failures BIGINT DEFAULT 0, paused_until BIGINT DEFAULT 0);
CREATE TABLE IF NOT EXISTS catalog.section_seats(inst TEXT COLLATE "C", term TEXT COLLATE "C", id TEXT COLLATE "C", capacity BIGINT NOT NULL, available BIGINT NOT NULL, waitlist BIGINT NOT NULL, checked_at BIGINT NOT NULL, changed_at BIGINT NOT NULL, PRIMARY KEY(inst,term,id));
CREATE TABLE IF NOT EXISTS catalog.catalog_revisions(inst TEXT COLLATE "C", term TEXT COLLATE "C", structure BIGINT NOT NULL DEFAULT 0, seats BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(inst,term));
"#;

/// Functions and triggers: the shared rate limiter, JSON helpers, catalog
/// revision counters, streak and reminder invalidation, and the account
/// notifications that wake event streams on other replicas.
const FUNCTIONS: &str = r#"
CREATE OR REPLACE FUNCTION rate_allow(identifier TEXT, budget INTEGER, cutoff BIGINT, stamp BIGINT, expiry BIGINT) RETURNS BOOLEAN LANGUAGE plpgsql AS $$
DECLARE retained BIGINT[];
BEGIN
 INSERT INTO rate_limits(key) VALUES(identifier) ON CONFLICT DO NOTHING;
 SELECT ARRAY(SELECT value::bigint FROM jsonb_array_elements_text(hits::jsonb) WHERE value::bigint>cutoff) INTO retained FROM rate_limits WHERE key=identifier FOR UPDATE;
 IF cardinality(retained)>=budget THEN RETURN false; END IF;
 UPDATE rate_limits SET hits=to_json(array_append(retained,stamp))::text,expires=expiry WHERE key=identifier;
 RETURN true;
END $$;

CREATE OR REPLACE FUNCTION rate_record(identifier TEXT, budget INTEGER, cutoff BIGINT, admitted BIGINT[], stamp BIGINT, expiry BIGINT) RETURNS INTEGER LANGUAGE plpgsql AS $$
DECLARE retained BIGINT[]; recorded INTEGER;
BEGIN
 INSERT INTO rate_limits(key) VALUES(identifier) ON CONFLICT DO NOTHING;
 SELECT ARRAY(SELECT value::bigint FROM jsonb_array_elements_text(hits::jsonb) WHERE value::bigint>cutoff) INTO retained FROM rate_limits WHERE key=identifier FOR UPDATE;
 retained:=retained||admitted;
 recorded:=cardinality(retained);
 IF recorded>=budget AND cardinality(admitted)=0 THEN RETURN recorded; END IF;
 IF recorded<budget THEN retained:=array_append(retained,stamp); END IF;
 UPDATE rate_limits SET hits=to_json(retained)::text,expires=expiry WHERE key=identifier;
 RETURN recorded;
END $$;

CREATE OR REPLACE FUNCTION json_valid(value TEXT) RETURNS BOOLEAN LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN PERFORM value::jsonb; RETURN value IS NOT NULL; EXCEPTION WHEN invalid_text_representation THEN RETURN false; END $$;
CREATE OR REPLACE FUNCTION json_text(value TEXT, path TEXT) RETURNS TEXT LANGUAGE plpgsql IMMUTABLE AS $$
BEGIN RETURN value::jsonb ->> substring(path FROM 3); EXCEPTION WHEN invalid_text_representation THEN RETURN NULL; END $$;
CREATE OR REPLACE FUNCTION utc_day(value BIGINT) RETURNS TEXT LANGUAGE sql IMMUTABLE AS $$
 SELECT to_char(to_timestamp(value) AT TIME ZONE 'UTC','YYYY-MM-DD') $$;
CREATE OR REPLACE FUNCTION catalog.revise_catalog() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE item record;
BEGIN
 IF TG_OP='DELETE' THEN item:=OLD; ELSE item:=NEW; END IF;
 IF TG_TABLE_NAME='sections' THEN
  IF TG_OP='UPDATE' AND OLD.data IS NOT DISTINCT FROM NEW.data THEN RETURN NEW; END IF;
  INSERT INTO catalog.catalog_revisions(inst,term,structure) VALUES(item.inst,item.term,1)
    ON CONFLICT(inst,term) DO UPDATE SET structure=catalog.catalog_revisions.structure+1;
  IF TG_OP='DELETE' THEN DELETE FROM catalog.section_seats WHERE inst=item.inst AND term=item.term AND id=item.id; END IF;
 ELSE
  IF TG_OP='UPDATE' AND (OLD.capacity,OLD.available,OLD.waitlist) IS NOT DISTINCT FROM (NEW.capacity,NEW.available,NEW.waitlist) THEN RETURN NEW; END IF;
  INSERT INTO catalog.catalog_revisions(inst,term,seats) VALUES(item.inst,item.term,1)
    ON CONFLICT(inst,term) DO UPDATE SET seats=catalog.catalog_revisions.seats+1;
 END IF;
 RETURN NULL;
END $$;
CREATE OR REPLACE TRIGGER catalog_structure AFTER INSERT OR UPDATE OR DELETE ON catalog.sections FOR EACH ROW EXECUTE FUNCTION catalog.revise_catalog();
CREATE OR REPLACE TRIGGER catalog_seats AFTER INSERT OR UPDATE OR DELETE ON catalog.section_seats FOR EACH ROW EXECUTE FUNCTION catalog.revise_catalog();
CREATE OR REPLACE FUNCTION invalidate_streak() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE account_id text;
BEGIN
 FOR account_id IN SELECT DISTINCT account FROM changed_docs LOOP
  DELETE FROM account_streak_cache WHERE account=account_id;
  DELETE FROM reminder_schedule WHERE id='acct:'||account_id;
  INSERT INTO work_jobs(kind,key,account,run_at) VALUES('reminder','acct:'||account_id,account_id,0)
    ON CONFLICT(kind,key) DO UPDATE SET generation=work_jobs.generation+1,run_at=0;
 END LOOP;
 RETURN NULL;
END $$;
CREATE OR REPLACE TRIGGER docs_streak_insert AFTER INSERT ON docs REFERENCING NEW TABLE AS changed_docs FOR EACH STATEMENT EXECUTE FUNCTION invalidate_streak();
CREATE OR REPLACE TRIGGER docs_streak_update AFTER UPDATE ON docs REFERENCING NEW TABLE AS changed_docs FOR EACH STATEMENT EXECUTE FUNCTION invalidate_streak();
CREATE OR REPLACE TRIGGER docs_streak_delete AFTER DELETE ON docs REFERENCING OLD TABLE AS changed_docs FOR EACH STATEMENT EXECUTE FUNCTION invalidate_streak();
CREATE OR REPLACE FUNCTION notify_account() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE account_id text;
BEGIN
 IF TG_TABLE_NAME='accounts' THEN
  IF TG_OP='UPDATE' AND OLD.seq=NEW.seq THEN RETURN NULL; END IF;
  IF TG_OP='DELETE' THEN account_id:=OLD.id; ELSE account_id:=NEW.id; END IF;
 ELSE
  IF TG_OP='DELETE' THEN account_id:=OLD.account; ELSE account_id:=NEW.account; END IF;
 END IF;
 IF account_id<>'' THEN PERFORM pg_notify('honorum_accounts',account_id); END IF;
 RETURN NULL;
END $$;
CREATE OR REPLACE TRIGGER accounts_changed AFTER UPDATE OR DELETE ON accounts FOR EACH ROW EXECUTE FUNCTION notify_account();
CREATE OR REPLACE TRIGGER watches_changed AFTER INSERT OR UPDATE OR DELETE ON watches FOR EACH ROW EXECUTE FUNCTION notify_account();
CREATE OR REPLACE TRIGGER google_changed AFTER INSERT OR UPDATE OR DELETE ON google_connections FOR EACH ROW EXECUTE FUNCTION notify_account();
CREATE OR REPLACE TRIGGER sessions_changed AFTER DELETE ON sessions FOR EACH ROW EXECUTE FUNCTION notify_account();
CREATE OR REPLACE FUNCTION queue_reminder_rebuild() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE identifier text; account_id text;
BEGIN
 IF TG_TABLE_NAME='reminders' THEN
  IF TG_OP='DELETE' THEN identifier:=OLD.id; ELSE identifier:=NEW.id; END IF;
  account_id:='';
 ELSE
  IF TG_TABLE_NAME='sessions' THEN
   IF TG_OP='UPDATE' THEN
    IF OLD.push=NEW.push THEN RETURN NULL; END IF;
   END IF;
  END IF;
  IF TG_OP='DELETE' THEN account_id:=OLD.account; ELSE account_id:=NEW.account; END IF;
  identifier:='acct:'||account_id;
 END IF;
 DELETE FROM reminder_schedule WHERE id=identifier;
 INSERT INTO work_jobs(kind,key,account,run_at) VALUES('reminder',identifier,account_id,0)
   ON CONFLICT(kind,key) DO UPDATE SET generation=work_jobs.generation+1,run_at=0;
 RETURN NULL;
END $$;
CREATE OR REPLACE TRIGGER reminder_registration AFTER INSERT OR UPDATE OR DELETE ON reminders FOR EACH ROW EXECUTE FUNCTION queue_reminder_rebuild();
CREATE OR REPLACE TRIGGER reminder_devices AFTER INSERT OR UPDATE OR DELETE ON sessions FOR EACH ROW EXECUTE FUNCTION queue_reminder_rebuild();
CREATE OR REPLACE TRIGGER reminder_timezone AFTER INSERT OR UPDATE OR DELETE ON account_timezones FOR EACH ROW EXECUTE FUNCTION queue_reminder_rebuild();
CREATE OR REPLACE FUNCTION delete_account_derivations() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 DELETE FROM work_jobs WHERE account=OLD.id;
 DELETE FROM reminder_schedule WHERE account=OLD.id;
 DELETE FROM account_streak_rollups WHERE account=OLD.id;
 DELETE FROM account_streak_cache WHERE account=OLD.id;
 RETURN NULL;
END $$;
CREATE OR REPLACE TRIGGER account_derivations AFTER DELETE ON accounts FOR EACH ROW EXECUTE FUNCTION delete_account_derivations();
"#;

pub(crate) async fn initialize(db: &Db) -> Result<()> {
    let mut client = db.client().await?;
    let tx = client.transaction().await?;
    tx.batch_execute("SELECT pg_advisory_xact_lock(726193018)").await?;
    tx.batch_execute("CREATE SCHEMA IF NOT EXISTS catalog").await?;
    tx.batch_execute(ACCOUNTS).await.map_err(|e| Error::Config(format!("account schema: {e}")))?;
    tx.batch_execute(CATALOG).await.map_err(|e| Error::Config(format!("catalog schema: {e}")))?;
    tx.batch_execute(FUNCTIONS).await.map_err(|e| Error::Config(format!("database functions: {e}")))?;
    tx.commit().await?;
    Ok(())
}
