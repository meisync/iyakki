//! S3-compatible object storage (MinIO from Quick install, or any server that speaks the S3 REST API) as a data source: the buckets
//! and the objects in them, with sizes and ages, as tables. Read only by construction: the only request this file ever builds is a
//! `GET` with an empty body (list buckets, list objects). Requests are signed with AWS Signature Version 4 (written here, with
//! SHA-256 from `sha2`; checked against the worked examples in the AWS documentation). The access key and the secret key are vault
//! references; neither is stored in the settings, shown, or kept in an error message. Without a key the requests go out unsigned
//! (a bucket that allows public listing).
//!
//! A query is the place to list: empty (or `/`) lists the buckets; `bucket` or `bucket/some/prefix` lists the keys under it, one
//! level at a time (folders as rows of kind `prefix`).

use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::connector::*;
use super::frame::*;

pub struct S3 {
    id: String,
    http: Http,
    region: String,
    access: Option<Zeroizing<String>>,
    secret: Option<Zeroizing<String>>,
    now_ms: fn() -> i64,
}

pub fn info() -> KindInfo {
    let mut fields: Vec<Field> = net_fields("http://localhost:9000").into_iter().filter(|f| !matches!(f.key.as_str(), "auth" | "username" | "password" | "token")).collect();
    fields.insert(1, Field::new("region", "Region", FieldKind::Text, "the signing region; MinIO and most S3-compatible servers accept us-east-1").default("us-east-1"));
    fields.insert(2, Field::new("access_key", "Access key", FieldKind::Secret, "vault reference of the access key id (leave empty for a bucket that lists without sign-in)"));
    fields.insert(3, Field::new("secret_key", "Secret key", FieldKind::Secret, "vault reference of the secret access key"));
    KindInfo {
        kind: "s3".into(),
        label: "Object storage (S3)".into(),
        language: Lang::S3,
        about: "Buckets and objects of an S3-compatible server (the object store of Quick install, or another), with sizes and ages as tables. Reads only: the source can list, never write or delete. Path-style addresses (http://host:9000/bucket).".into(),
        fields,
        example: "http://localhost:9000 (own server) or cluster://iyakki-data/minio:9000 (Quick install, runs inside the cluster)".into(),
    }
}

// ------------------------------------------------------------------------------------------------------ signing

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

/// SHA-256 of nothing: the payload hash of every request here.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// URI-encode as SigV4 wants: letters, digits and `-_.~` stay, everything else is `%XX` (capitals); `/` stays only in a path.
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub struct Signed {
    pub authorization: String,
    pub canonical_request: String,
    pub signature: String,
}

/// One request, signed. `path` is the URI path as it goes on the wire (already encoded), `query` the raw pairs, `headers` every
/// header that is to be signed (lower-case names, including `host` and the `x-amz-*` ones).
#[allow(clippy::too_many_arguments)]
pub fn sign_v4(access: &str, secret: &str, region: &str, service: &str, method: &str, path: &str, query: &[(&str, &str)], headers: &[(&str, &str)], payload_hash: &str, amz_date: &str) -> Signed {
    let mut q: Vec<(String, String)> = query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
    q.sort();
    let canonical_query = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let mut h: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    h.sort();
    let canonical_headers: String = h.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = h.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical_request = format!("{method}\n{path}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let day = &amz_date[..8];
    let scope = format!("{day}/{region}/{service}/aws4_request");
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));
    let hm = crate::s3::hmac_sha256;
    let k_date = hm(format!("AWS4{secret}").as_bytes(), day.as_bytes());
    let k_region = hm(&k_date, region.as_bytes());
    let k_service = hm(&k_region, service.as_bytes());
    let k_signing = hm(&k_service, b"aws4_request");
    let signature = hex(&hm(&k_signing, to_sign.as_bytes()));
    Signed { authorization: format!("AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"), canonical_request, signature }
}

/// HMAC-SHA256 (RFC 2104) over SHA-256 from `sha2`.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.iter().map(|b| b ^ 0x36).collect::<Vec<u8>>());
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(k.iter().map(|b| b ^ 0x5c).collect::<Vec<u8>>());
    outer.update(ih);
    outer.finalize().into()
}

/// `20130524T000000Z` from seconds since the epoch.
pub fn amz_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", t / 3600, (t % 3600) / 60, t % 60)
}

fn real_now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ------------------------------------------------------------------------------------------------------ the answer (XML)

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            e if e.starts_with("#x") => out.push(u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32).unwrap_or('?')),
            e if e.starts_with('#') => out.push(e[1..].parse::<u32>().ok().and_then(char::from_u32).unwrap_or('?')),
            _ => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The inner text of every `<tag>...</tag>` in `xml` (no nesting of the same tag, which S3 answers do not have).
fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = vec![];
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        let Some(j) = after.find(&close) else { break };
        out.push(&after[..j]);
        rest = &after[j + close.len()..];
    }
    out
}

fn text_of(xml: &str, tag: &str) -> Option<String> {
    blocks(xml, tag).first().map(|s| unescape(s))
}

pub struct Listing {
    /// (name, created ms)
    pub buckets: Vec<(String, Option<i64>)>,
    pub prefixes: Vec<String>,
    /// (key, size, modified ms, storage class)
    pub objects: Vec<(String, i64, Option<i64>, String)>,
    pub next_token: Option<String>,
}

pub fn parse_listing(xml: &str) -> Result<Listing, DashError> {
    if !(xml.contains("<ListBucketResult") || xml.contains("<ListAllMyBucketsResult")) {
        return Err(DashError::parse("the answer is not an S3 listing (is the address an S3 server?)"));
    }
    let buckets = blocks(xml, "Bucket").into_iter().filter_map(|b| Some((text_of(b, "Name")?, text_of(b, "CreationDate").and_then(|d| parse_time_ms(&d))))).collect();
    let prefixes = blocks(xml, "CommonPrefixes").into_iter().filter_map(|b| text_of(b, "Prefix")).collect();
    let objects = blocks(xml, "Contents")
        .into_iter()
        .filter_map(|b| Some((text_of(b, "Key")?, text_of(b, "Size").and_then(|s| s.trim().parse().ok()).unwrap_or(0), text_of(b, "LastModified").and_then(|d| parse_time_ms(&d)), text_of(b, "StorageClass").unwrap_or_default())))
        .collect();
    let truncated = text_of(xml, "IsTruncated").is_some_and(|t| t.trim() == "true");
    let next_token = if truncated { text_of(xml, "NextContinuationToken") } else { None };
    Ok(Listing { buckets, prefixes, objects, next_token })
}

// ------------------------------------------------------------------------------------------------------ the source

/// A bucket name as S3 allows it (3 to 63 of lower-case letters, digits, `.` and `-`; shorter ones are allowed here for servers that do).
pub fn valid_bucket(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-')) && !s.starts_with(['.', '-'])
}

/// `(bucket, prefix)` from a query; `None` for the bucket list.
pub fn parse_place(text: &str) -> Result<Option<(String, String)>, DashError> {
    let t = text.trim().trim_start_matches('/');
    if t.is_empty() {
        return Ok(None);
    }
    let (bucket, prefix) = t.split_once('/').unwrap_or((t, ""));
    if !valid_bucket(bucket) {
        return Err(DashError::new(ErrKind::BadQuery, "that is not a bucket name").fix("Write the bucket (lower-case letters, digits, . and -) and, if you like, a prefix: bucket/folder/. An empty query lists the buckets."));
    }
    if prefix.len() > 1024 || prefix.chars().any(char::is_control) {
        return Err(DashError::new(ErrKind::BadQuery, "that prefix is not usable (up to 1024 characters, no control characters)"));
    }
    Ok(Some((bucket.to_string(), prefix.to_string())))
}

impl S3 {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<S3, DashError> {
        let what = format!("S3 “{}”", cfg.name);
        let mut http = Http::from_config(cfg, d, &what)?;
        let get = |key: &str| -> Result<Option<Zeroizing<String>>, DashError> {
            match cfg.secrets.get(key).filter(|r| !r.is_empty()) {
                None => Ok(None),
                Some(r) => {
                    if !valid_secret_ref(r) {
                        return Err(DashError::config(format!("{what}: the {key} must be a vault reference like uvault:name")));
                    }
                    d.secrets.resolve(r).map(|v| Some(Zeroizing::new(v.trim().to_string()))).map_err(|e| vault_error(&what, key, r, &e))
                }
            }
        };
        let (access, secret) = (get("access_key")?, get("secret_key")?);
        if access.is_some() != secret.is_some() {
            return Err(DashError::config(format!("{what}: give both the access key and the secret key, or neither")));
        }
        for s in access.iter().chain(secret.iter()) {
            http.add_secret(s.clone());
        }
        let region = Some(cfg.value("region")).filter(|s| !s.is_empty()).unwrap_or("us-east-1").to_string();
        if !region.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || region.len() > 30 {
            return Err(DashError::config(format!("{what}: `{}` is not a region name", clip(&region, 30))));
        }
        Ok(S3 { id: cfg.id.clone(), http, region, access, secret, now_ms: real_now_ms })
    }

    #[cfg(test)]
    fn with_clock(mut self, now_ms: fn() -> i64) -> S3 {
        self.now_ms = now_ms;
        self
    }

    fn host(&self) -> String {
        let b = self.http.base();
        b.split_once("://").map(|x| x.1).unwrap_or(b).split('/').next().unwrap_or("").to_string()
    }

    /// One signed (or, without a key, plain) GET. `path` is the raw path (`/bucket/`), `query` the raw pairs.
    fn get(&self, path: &str, query: &[(&str, &str)], cancel: &Cancel) -> Result<String, DashError> {
        let wire_path = uri_encode(path, true);
        let mut q: Vec<(String, String)> = query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
        q.sort();
        let target = if q.is_empty() { wire_path.clone() } else { format!("{wire_path}?{}", q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")) };
        let mut extra: Vec<(String, String)> = vec![];
        if let (Some(a), Some(s)) = (&self.access, &self.secret) {
            let date = amz_date((self.now_ms)() / 1000);
            let host = self.host();
            let headers = [("host", host.as_str()), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", date.as_str())];
            let signed = sign_v4(a, s, &self.region, "s3", "GET", &wire_path, query, &headers, EMPTY_SHA256, &date);
            extra.push(("x-amz-content-sha256".into(), EMPTY_SHA256.into()));
            extra.push(("x-amz-date".into(), date));
            extra.push(("Authorization".into(), signed.authorization));
        }
        let extra_ref: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let r = self.http.send("GET", &target, &extra_ref, &[], cancel)?;
        Ok(String::from_utf8_lossy(&r.body).into_owned())
    }

    fn list_buckets(&self, cancel: &Cancel) -> Result<Listing, DashError> {
        parse_listing(&self.get("/", &[], cancel)?)
    }

    /// The keys under a prefix, one level (folders come back as prefixes), following continuation tokens until `limit` rows.
    fn list_objects(&self, bucket: &str, prefix: &str, limit: usize, cancel: &Cancel) -> Result<(Listing, bool), DashError> {
        let mut all = Listing { buckets: vec![], prefixes: vec![], objects: vec![], next_token: None };
        let mut token: Option<String> = None;
        for _ in 0..10 {
            let max = (limit.saturating_sub(all.prefixes.len() + all.objects.len())).clamp(1, 1000).to_string();
            let mut q: Vec<(&str, &str)> = vec![("list-type", "2"), ("delimiter", "/"), ("max-keys", &max), ("encoding-type", "url")];
            if !prefix.is_empty() {
                q.push(("prefix", prefix));
            }
            if let Some(t) = &token {
                q.push(("continuation-token", t));
            }
            let l = parse_listing(&self.get(&format!("/{bucket}"), &q, cancel)?)?;
            all.prefixes.extend(l.prefixes);
            all.objects.extend(l.objects);
            token = l.next_token;
            if token.is_none() || all.prefixes.len() + all.objects.len() >= limit {
                break;
            }
        }
        let more = token.is_some() || all.prefixes.len() + all.objects.len() > limit;
        Ok((all, more))
    }

    fn age_days(&self, modified: Option<i64>) -> Value {
        match modified {
            Some(m) => Value::from((((self.now_ms)() - m).max(0) as f64 / 86_400_000.0 * 10.0).round() / 10.0),
            None => Value::Null,
        }
    }
}

fn col(name: &str, ty: ColType) -> Column {
    Column { name: name.into(), ty }
}

/// With `encoding-type=url` S3 percent-encodes keys and prefixes in the answer.
fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() + 0 && b.get(i + 1).zip(b.get(i + 2)).is_some_and(|(x, y)| x.is_ascii_hexdigit() && y.is_ascii_hexdigit()) => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl DataSource for S3 {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "s3"
    }
    fn language(&self) -> Lang {
        Lang::S3
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        let l = self.list_buckets(cancel)?;
        Ok(TestInfo { ok: true, detail: format!("{} bucket{}", l.buckets.len(), if l.buckets.len() == 1 { "" } else { "s" }) })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        match path {
            [] => Ok(self.list_buckets(cancel)?.buckets.into_iter().map(|(name, _)| Node { name, kind: NodeKind::Database, detail: "bucket".into(), has_children: true }).collect()),
            [bucket] => {
                if !valid_bucket(bucket) {
                    return Err(DashError::new(ErrKind::BadQuery, "that is not a bucket name"));
                }
                let (l, _) = self.list_objects(bucket, "", 200, cancel)?;
                let mut out: Vec<Node> = l.prefixes.iter().map(|p| Node { name: url_decode(p), kind: NodeKind::Table, detail: "folder".into(), has_children: false }).collect();
                out.extend(l.objects.iter().map(|(k, size, _, _)| Node { name: url_decode(k), kind: NodeKind::File, detail: format!("{size} bytes"), has_children: false }));
                Ok(out)
            }
            _ => Ok(vec![]),
        }
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let started = std::time::Instant::now();
        let limit = req.limit();
        let mut f = Frame::default();
        let place = parse_place(&req.text)?;
        match &place {
            None => {
                let l = self.list_buckets(cancel)?;
                f.columns = vec![col("bucket", ColType::Str), col("created", ColType::Time), col("age_days", ColType::Float)];
                f.rows = l.buckets.into_iter().map(|(n, c)| vec![Value::from(n), c.map(Value::from).unwrap_or(Value::Null), self.age_days(c)]).collect();
            }
            Some((bucket, prefix)) => {
                let (l, more) = self.list_objects(bucket, prefix, limit, cancel)?;
                f.columns = vec![col("key", ColType::Str), col("kind", ColType::Str), col("size_bytes", ColType::Int), col("modified", ColType::Time), col("age_days", ColType::Float), col("storage_class", ColType::Str)];
                for p in &l.prefixes {
                    f.rows.push(vec![Value::from(url_decode(p)), Value::from("prefix"), Value::Null, Value::Null, Value::Null, Value::Null]);
                }
                for (k, size, m, class) in &l.objects {
                    f.rows.push(vec![Value::from(url_decode(k)), Value::from("object"), Value::from(*size), m.map(Value::from).unwrap_or(Value::Null), self.age_days(*m), Value::from(class.as_str())]);
                }
                if more {
                    f.meta.truncated = true;
                }
            }
        }
        f.cap(limit, MAX_SERIES, MAX_POINTS);
        if f.meta.truncated {
            f.meta.notices.push(format!("the listing was cut at {limit} rows"));
        }
        f.meta.took_ms = took(started);
        f.meta.executed = match place {
            None => "list buckets".to_string(),
            Some((b, p)) => format!("list {b}/{p}"),
        };
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        if valid_bucket(target) { target.to_string() } else { String::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::fake::*;
    use iyakki_shared::wsexec::FakeRunner;
    use serde_json::json;
    use std::sync::Arc;

    /// 2026-11-13 12:00:00 UTC
    fn clock() -> i64 {
        1_794_571_200_000
    }

    fn cfg_with_keys() -> SourceConfig {
        let mut c = cfg("s3", "http://store:9000");
        c.secrets.insert("access_key".into(), "uvault:s3.access".into());
        c.secrets.insert("secret_key".into(), "uvault:s3.secret".into());
        c
    }

    fn src(up: &Arc<FakeUp>, c: &SourceConfig) -> S3 {
        let d = deps(up, &[("uvault:s3.access", "AKIDEXAMPLE0001"), ("uvault:s3.secret", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")], Arc::new(FakeRunner::new(&[])));
        S3::new(c, &d).unwrap().with_clock(clock)
    }

    const BUCKETS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><ListAllMyBucketsResult><Owner><ID>x</ID></Owner><Buckets><Bucket><Name>photos</Name><CreationDate>2026-11-03T12:00:00.000Z</CreationDate></Bucket><Bucket><Name>backups</Name><CreationDate>2026-10-14T12:00:00.000Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>"#;
    const OBJECTS: &str = r#"<ListBucketResult><Name>photos</Name><Prefix>2026/</Prefix><IsTruncated>false</IsTruncated><Contents><Key>2026/a%26b.jpg</Key><LastModified>2026-11-12T12:00:00.000Z</LastModified><Size>2048</Size><StorageClass>STANDARD</StorageClass></Contents><Contents><Key>2026/c.png</Key><LastModified>2026-11-06T12:00:00.000Z</LastModified><Size>10</Size><StorageClass>STANDARD</StorageClass></Contents><CommonPrefixes><Prefix>2026/trip%20one/</Prefix></CommonPrefixes></ListBucketResult>"#;

    // The worked examples of the AWS documentation (Signature Version 4, "Examples: Signature Calculations in Authorization Header").
    const AWS_ACCESS: &str = "AKIAIOSFODNN7EXAMPLE";
    const AWS_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    #[test]
    fn the_signature_matches_the_aws_get_object_example() {
        let h = [("host", "examplebucket.s3.amazonaws.com"), ("range", "bytes=0-9"), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", "20130524T000000Z")];
        let s = sign_v4(AWS_ACCESS, AWS_SECRET, "us-east-1", "s3", "GET", "/test.txt", &[], &h, EMPTY_SHA256, "20130524T000000Z");
        assert_eq!(sha256_hex(s.canonical_request.as_bytes()), "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972");
        assert_eq!(s.signature, "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41");
        assert!(s.authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature="));
    }

    #[test]
    fn the_signature_matches_the_aws_list_objects_example() {
        let h = [("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY_SHA256), ("x-amz-date", "20130524T000000Z")];
        let s = sign_v4(AWS_ACCESS, AWS_SECRET, "us-east-1", "s3", "GET", "/", &[("max-keys", "2"), ("prefix", "J")], &h, EMPTY_SHA256, "20130524T000000Z");
        assert_eq!(s.signature, "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7");
    }

    #[test]
    fn encoding_and_dates_follow_the_rules() {
        assert_eq!(uri_encode("a b/c~d_e-f.g", true), "a%20b/c~d_e-f.g");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("é&=", false), "%C3%A9%26%3D");
        assert_eq!(amz_date(1_369_353_600), "20130524T000000Z");
        assert_eq!(amz_date(1_794_571_200), "20261113T120000Z");
    }

    #[test]
    fn buckets_are_a_table_with_ages() {
        let up = FakeUp::new().on("GET http://store:9000/", 200, BUCKETS);
        let f = src(&up, &cfg_with_keys()).query(&QueryRequest::new("", range()), &Cancel::new()).unwrap();
        assert_eq!(f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["bucket", "created", "age_days"]);
        assert_eq!((f.cell(0, 0), f.cell(0, 2)), ("photos".to_string(), "10.0".to_string()));
        assert_eq!(f.cell(1, 2), "30.0");
    }

    #[test]
    fn objects_come_with_sizes_ages_and_folders_and_keys_are_decoded() {
        let up = FakeUp::new().on("GET http://store:9000/photos?", 200, OBJECTS);
        let f = src(&up, &cfg_with_keys()).query(&QueryRequest::new("photos/2026/", range()), &Cancel::new()).unwrap();
        assert_eq!(f.rows.len(), 3);
        assert_eq!((f.cell(0, 0), f.cell(0, 1)), ("2026/trip one/".to_string(), "prefix".to_string()));
        assert_eq!((f.cell(1, 0), f.cell(1, 2), f.cell(1, 4)), ("2026/a&b.jpg".to_string(), "2048".to_string(), "1.0".to_string()));
        let url = up.urls().remove(0);
        assert!(url.contains("/photos?continuation") == false && url.contains("delimiter=%2F") && url.contains("list-type=2") && url.contains("prefix=2026%2F"), "{url}");
    }

    #[test]
    fn every_request_is_a_signed_get_with_no_body_and_no_key_leaves_the_machine() {
        let up = FakeUp::new().on("GET", 200, OBJECTS);
        let s = src(&up, &cfg_with_keys());
        let _ = s.query(&QueryRequest::new("photos", range()), &Cancel::new()).unwrap();
        let _ = s.browse(&["photos".to_string()], &Cancel::new()).unwrap();
        let seen = up.seen.lock().unwrap();
        assert!(!seen.is_empty());
        for (method, _, headers, body) in seen.iter() {
            assert_eq!(method, "GET");
            assert!(body.is_empty());
            let auth = headers.iter().find(|h| h.0 == "Authorization").map(|h| h.1.clone()).unwrap();
            assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE0001/20261113/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="), "{auth}");
            assert!(headers.iter().any(|h| h.0 == "x-amz-date" && h.1 == "20261113T120000Z"));
            assert!(!format!("{headers:?}").contains("wJalrXUtnFEMI"), "the secret key is never sent");
        }
    }

    #[test]
    fn without_a_key_the_request_is_plain_and_one_key_alone_is_refused() {
        let up = FakeUp::new().on("GET", 200, BUCKETS);
        let d = deps(&up, &[("uvault:s3.access", "AKIDEXAMPLE0001")], Arc::new(FakeRunner::new(&[])));
        let s = S3::new(&cfg("s3", "http://store:9000"), &d).unwrap();
        s.test(&Cancel::new()).unwrap();
        assert!(up.seen.lock().unwrap()[0].2.iter().all(|h| h.0 != "Authorization"));
        let mut c = cfg("s3", "http://store:9000");
        c.secrets.insert("access_key".into(), "uvault:s3.access".into());
        let e = S3::new(&c, &d).err().unwrap();
        assert!(e.message.contains("both the access key and the secret key"), "{e}");
    }

    #[test]
    fn a_follow_up_page_is_asked_for_until_the_limit_and_a_cut_is_said() {
        let first = r#"<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>tok/1+=</NextContinuationToken><Contents><Key>a</Key><Size>1</Size></Contents><Contents><Key>b</Key><Size>2</Size></Contents></ListBucketResult>"#;
        let up = FakeUp::new().on("GET http://store:9000/big?", 200, first);
        let mut r = QueryRequest::new("big", range());
        r.limit = 2;
        let f = src(&up, &cfg_with_keys()).query(&r, &Cancel::new()).unwrap();
        assert_eq!(f.rows.len(), 2);
        assert!(f.meta.truncated && f.meta.notices.iter().any(|n| n.contains("cut at 2")));
        // with room left, the continuation token is sent encoded
        let up = FakeUp::new().on("continuation-token", 200, r#"<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>c</Key><Size>3</Size></Contents></ListBucketResult>"#).on("GET", 200, first);
        let f = src(&up, &cfg_with_keys()).query(&QueryRequest::new("big", range()), &Cancel::new()).unwrap();
        assert_eq!(f.rows.len(), 3);
        assert!(up.urls()[1].contains("continuation-token=tok%2F1%2B%3D"), "{:?}", up.urls());
    }

    #[test]
    fn bad_places_and_bad_answers_are_said_in_words_and_keys_never_appear_in_errors() {
        let up = FakeUp::new();
        let s = src(&up, &cfg_with_keys());
        for q in ["Bad_Bucket", "../etc", "-x", "a b"] {
            assert_eq!(s.query(&QueryRequest::new(q, range()), &Cancel::new()).unwrap_err().kind, ErrKind::BadQuery, "{q}");
        }
        assert!(up.urls().is_empty(), "nothing was sent for a bad name");
        let up = FakeUp::new().on("GET", 403, "<Error><Code>SignatureDoesNotMatch</Code><Message>AKIDEXAMPLE0001 wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY</Message></Error>");
        let e = src(&up, &cfg_with_keys()).test(&Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::Auth);
        assert!(!format!("{e}").contains("AKIDEXAMPLE0001") && !format!("{e:?}").contains("wJalrXUtnFEMI"), "{e}");
        let up = FakeUp::new().on("GET", 200, "<html>hello</html>");
        assert_eq!(src(&up, &cfg_with_keys()).test(&Cancel::new()).unwrap_err().kind, ErrKind::Parse);
    }

    #[test]
    fn the_listing_parser_handles_entities_and_empty_answers() {
        let l = parse_listing("<ListBucketResult><Contents><Key>a&amp;b&lt;c&#65;</Key><Size>5</Size></Contents></ListBucketResult>").unwrap();
        assert_eq!(l.objects[0].0, "a&b<cA");
        assert!(parse_listing("<ListBucketResult></ListBucketResult>").unwrap().objects.is_empty());
        assert_eq!(url_decode("a%20b+c%2"), "a b c%2");
    }

    #[test]
    fn the_form_hides_nothing_secret_in_values() {
        let i = info();
        assert!(i.fields.iter().filter(|f| f.key == "access_key" || f.key == "secret_key").all(|f| matches!(f.kind, FieldKind::Secret)));
        assert!(i.fields.iter().all(|f| !matches!(f.key.as_str(), "auth" | "password" | "token")));
    }

    /// A second implementation written from the AWS description in Python's standard library (hmac, hashlib, urllib): the same requests are
    /// signed by both and must give the same signature. Skipped (with a printed reason) where `python3` is not on the machine.
    #[test]
    fn the_signature_equals_an_independent_python_implementation_for_awkward_requests() {
        const PY: &str = r#"
import hashlib, hmac, json, sys
from urllib.parse import quote
def enc(s, slash): return quote(s, safe="-_.~/" if slash else "-_.~")
def sign(a):
    q = sorted((enc(k, False), enc(v, False)) for k, v in a["query"])
    cq = "&".join(k + "=" + v for k, v in q)
    h = sorted((k.lower(), v.strip()) for k, v in a["headers"])
    ch = "".join(k + ":" + v + "\n" for k, v in h)
    sh = ";".join(k for k, v in h)
    cr = "\n".join([a["method"], a["path"], cq, ch, sh, a["payload"]])
    day = a["date"][:8]
    scope = "/".join([day, a["region"], "s3", "aws4_request"])
    sts = "\n".join(["AWS4-HMAC-SHA256", a["date"], scope, hashlib.sha256(cr.encode()).hexdigest()])
    k = ("AWS4" + a["secret"]).encode()
    for part in [day, a["region"], "s3", "aws4_request"]:
        k = hmac.new(k, part.encode(), hashlib.sha256).digest()
    return hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
print(json.dumps([sign(a) for a in json.load(sys.stdin)]))
"#;
        let empty = EMPTY_SHA256;
        let cases: Vec<(&str, &str, &str, Vec<(&str, &str)>, &str)> = vec![
            ("GET", "/", "us-east-1", vec![], "20261113T120000Z"),
            ("GET", "/photos", "eu-west-2", vec![("list-type", "2"), ("delimiter", "/"), ("prefix", "2026/trip one/"), ("max-keys", "1000")], "20260229T235959Z"),
            ("GET", "/b-1.x", "us-east-1", vec![("prefix", "a&b=c+d~e"), ("continuation-token", "tok/1+=")], "20300101T000000Z"),
            ("GET", "/photos", "ap-southeast-1", vec![("prefix", "caf\u{e9} \u{2603}/"), ("encoding-type", "url")], "20261231T010203Z"),
        ];
        let mut args = vec![];
        let mut mine = vec![];
        for (m, path, region, q, date) in &cases {
            let host = "store.example:9000";
            let headers = [("host", host), ("x-amz-content-sha256", empty), ("x-amz-date", *date)];
            let wire = uri_encode(path, true);
            mine.push(sign_v4("AKIDEXAMPLE", "s3cr3t/Key+with=chars", region, "s3", m, &wire, q, &headers, empty, date).signature);
            args.push(json!({"method": m, "path": wire, "region": region, "date": date, "secret": "s3cr3t/Key+with=chars", "payload": empty,
                "query": q.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                "headers": headers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>()}));
        }
        let child = std::process::Command::new("python3").args(["-c", PY]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
        let Ok(mut child) = child else {
            eprintln!("skipped: python3 is not on this machine");
            return;
        };
        use std::io::Write;
        child.stdin.take().unwrap().write_all(Value::Array(args).to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let theirs: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(theirs, mine, "the two implementations agree on every request");
        assert_eq!(theirs.len(), 4);
    }
}
