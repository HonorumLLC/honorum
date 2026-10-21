//! Catalog object storage: published snapshot objects are uploaded to an
//! S3-compatible bucket (Cloudflare R2, path-style, region `auto`, AWS
//! Signature Version 4) under `honorum-public/catalog/INST/FILE` before the
//! manifest switches.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::Server;

/// The R2 bucket client and the keys already uploaded by this process
/// (forgotten after 2000).
pub(crate) struct CatalogObjects {
    pub(crate) client: reqwest::Client,
    pub(crate) bucket: String,
    pub(crate) endpoint: String,
    pub(crate) access_key: String,
    pub(crate) secret_key: String,
    pub(crate) uploaded: Mutex<HashSet<String>>,
}

impl CatalogObjects {
    /// The store configured by `HONORUM_CATALOG_OBJECTS=1` with
    /// `HONORUM_R2_BUCKET`, `HONORUM_R2_ENDPOINT`, `HONORUM_R2_ACCESS_KEY_ID`,
    /// and `HONORUM_R2_SECRET_ACCESS_KEY`, read through `env`. `None` unless
    /// enabled; fails with "catalog objects require R2 bucket, endpoint and
    /// credentials" when enabled without all four.
    pub(crate) fn from_env(env: impl Fn(&str) -> String) -> anyhow::Result<Option<Self>> {
        if env("HONORUM_CATALOG_OBJECTS") != "1" {
            return Ok(None);
        }
        let (bucket, endpoint, access_key, secret_key) = (
            env("HONORUM_R2_BUCKET"),
            env("HONORUM_R2_ENDPOINT"),
            env("HONORUM_R2_ACCESS_KEY_ID"),
            env("HONORUM_R2_SECRET_ACCESS_KEY"),
        );
        if bucket.is_empty() || endpoint.is_empty() || access_key.is_empty() || secret_key.is_empty() {
            anyhow::bail!("catalog objects require R2 bucket, endpoint and credentials");
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("the object store client configuration is valid");
        Ok(Some(Self { client, bucket, endpoint, access_key, secret_key, uploaded: Mutex::new(HashSet::new()) }))
    }
}

impl Server {
    /// Uploads one object's three encodings (`application/json`, with
    /// `Content-Encoding` gzip/br, immutable cache policy), unless object
    /// storage is off, `hash` is empty, or this process already uploaded it.
    /// A failure ("catalog object upload: …") leaves clients on the previous
    /// publication, since the manifest switches only after every upload.
    pub(crate) async fn upload_snapshot_files(
        &self,
        inst: &str,
        term: &str,
        kind: &str,
        hash: &str,
    ) -> anyhow::Result<()> {
        let Some(store) = &self.objects else { return Ok(()) };
        if hash.is_empty() {
            return Ok(());
        }
        let base = format!("{term}.{kind}.{hash}.json");
        let key_base = format!("honorum-public/catalog/{inst}/{base}");
        if store.uploaded.lock().expect("uploaded lock").contains(&key_base) {
            return Ok(());
        }
        let dir = self.data_dir.join("catalog").join(inst);
        for (suffix, encoding) in [("", ""), (".gz", "gzip"), (".br", "br")] {
            let body = tokio::fs::read(dir.join(format!("{base}{suffix}"))).await?;
            let key = format!("{key_base}{suffix}");
            match tokio::time::timeout(Duration::from_secs(30), store.put(&key, body, encoding)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => anyhow::bail!("catalog object upload: {err}"),
                Err(_) => anyhow::bail!("catalog object upload: context deadline exceeded"),
            }
        }
        let mut uploaded = store.uploaded.lock().expect("uploaded lock");
        if uploaded.len() > 2000 {
            uploaded.clear();
        }
        uploaded.insert(key_base);
        Ok(())
    }
}

/// Characters S3 leaves unescaped in a path segment: RFC 3986 unreserved.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

impl CatalogObjects {
    /// Stores one object with a path-style, SigV4-signed `PUT`.
    async fn put(&self, key: &str, body: Vec<u8>, encoding: &str) -> anyhow::Result<()> {
        let path = format!("/{}/{}", self.bucket, key);
        let canonical_uri: String =
            path.split('/').map(|s| utf8_percent_encode(s, SEGMENT).to_string()).collect::<Vec<_>>().join("/");
        let url = url::Url::parse(&format!("{}{canonical_uri}", self.endpoint.trim_end_matches('/')))?;
        let host = match (url.host_str(), url.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            _ => anyhow::bail!("invalid R2 endpoint"),
        };
        let payload = hex::encode(Sha256::digest(&body));
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let mut headers = vec![
            ("cache-control", "public, max-age=31536000, immutable".to_owned()),
            ("content-type", "application/json".to_owned()),
            ("host", host),
            ("x-amz-content-sha256", payload.clone()),
            ("x-amz-date", amz_date),
        ];
        // The AWS SDK omits an empty Content-Encoding.
        if !encoding.is_empty() {
            headers.push(("content-encoding", encoding.to_owned()));
        }
        headers.sort();
        let authorization =
            sign_v4(&self.access_key, &self.secret_key, now, "auto", "PUT", url.path(), "", &headers, &payload);
        let mut request = self.client.put(url).body(body).header("authorization", authorization);
        for (name, value) in &headers {
            if *name != "host" {
                request = request.header(*name, value);
            }
        }
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            // Provider bodies are not logged or returned.
            anyhow::bail!("PutObject: status {}", status.as_u16());
        }
        Ok(())
    }
}

/// The `Authorization` value for an S3 request signed with AWS Signature
/// Version 4. `headers` are lowercase names with trimmed values, sorted by
/// name, and must include `host` and `x-amz-date`; `payload` is the hex
/// SHA-256 of the body (or `UNSIGNED-PAYLOAD`).
fn sign_v4(
    access_key: &str,
    secret_key: &str,
    at: DateTime<Utc>,
    region: &str,
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(&str, String)],
    payload: &str,
) -> String {
    let date = at.format("%Y%m%d").to_string();
    let amz_date = at.format("%Y%m%dT%H%M%SZ").to_string();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed_headers = headers.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
    let canonical_request =
        format!("{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload}");
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex::encode(Sha256::digest(canonical_request.as_bytes())));
    let mut key = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), date.as_bytes());
    for part in [region, "s3", "aws4_request"] {
        key = hmac_sha256(&key, part.as_bytes());
    }
    let signature = hex::encode(hmac_sha256(&key, string_to_sign.as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_object_configuration() {
        let settings = [
            ("HONORUM_R2_BUCKET", "catalog-test"),
            ("HONORUM_R2_ENDPOINT", "https://r2.example"),
            ("HONORUM_R2_ACCESS_KEY_ID", "test-key"),
            ("HONORUM_R2_SECRET_ACCESS_KEY", "test-secret"),
        ];
        let env = |enabled: &'static str, missing: Option<&'static str>| {
            move |key: &str| -> String {
                if key == "HONORUM_CATALOG_OBJECTS" {
                    return enabled.to_owned();
                }
                if enabled != "1" || missing == Some(key) {
                    return String::new();
                }
                settings.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()).unwrap_or_default()
            }
        };
        let disabled = CatalogObjects::from_env(env("0", None));
        assert!(matches!(disabled, Ok(None)), "disabled publisher");
        for (name, _) in settings {
            assert!(
                CatalogObjects::from_env(env("1", Some(name))).is_err(),
                "incomplete publication settings accepted: {name}"
            );
        }
        let store = CatalogObjects::from_env(env("1", None)).expect("configured publisher").expect("a publisher");
        assert_eq!(store.bucket, "catalog-test");
    }

    /// The example request in AWS's Signature Version 4 documentation for S3
    /// (`GET /test.txt` with a `Range` header).
    #[test]
    fn signature_matches_the_aws_example() {
        let at = DateTime::parse_from_rfc3339("2013-05-24T00:00:00Z").unwrap().with_timezone(&Utc);
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let headers = vec![
            ("host", "examplebucket.s3.amazonaws.com".to_owned()),
            ("range", "bytes=0-9".to_owned()),
            ("x-amz-content-sha256", empty.to_owned()),
            ("x-amz-date", "20130524T000000Z".to_owned()),
        ];
        let authorization = sign_v4(
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            at,
            "us-east-1",
            "GET",
            "/test.txt",
            "",
            &headers,
            empty,
        );
        assert_eq!(
            authorization,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }
}
