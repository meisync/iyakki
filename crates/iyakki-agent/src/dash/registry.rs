//! The list of kinds of data source and the one place that builds a connector from a saved config. To add a connector: write
//! its file next to this one (a struct that implements [`DataSource`], an `info()` and a `new(cfg, deps)`), declare the module in
//! `mod.rs`, and add one line to [`kinds`] and one to [`build`]. The add-source form, the assistant's tools and the tests
//! pick it up from there.

use super::connector::*;

/// Every kind this build offers, in the order the form shows them.
pub fn kinds() -> Vec<KindInfo> {
    vec![super::prom::info(), super::es::info(), super::clickhouse::info(), super::duckdb::info()]
}

pub fn kind(name: &str) -> Option<KindInfo> {
    kinds().into_iter().find(|k| k.kind == name)
}

/// Build the connector for a config. Fails with a sentence when the config is wrong or a secret cannot be read now.
pub fn build(cfg: &SourceConfig, d: &Deps) -> Result<Box<dyn DataSource>, DashError> {
    let info = kind(&cfg.kind).ok_or_else(|| DashError::config(format!("unknown kind of source `{}`", cfg.kind)))?;
    let problems = validate(cfg, &info);
    if !problems.is_empty() {
        return Err(DashError::config(problems.join("; ")));
    }
    Ok(match cfg.kind.as_str() {
        "prometheus" => Box::new(super::prom::Prometheus::new(cfg, d)?),
        "elasticsearch" => Box::new(super::es::Elastic::new(cfg, d)?),
        "clickhouse" => Box::new(super::clickhouse::ClickHouse::new(cfg, d)?),
        "duckdb" => Box::new(super::duckdb::DuckDb::new(cfg, d)?),
        other => return Err(DashError::config(format!("the kind `{other}` has no connector in this build"))),
    })
}

/// Kinds that are designed for but not built yet (shown in the docs and as "coming" in the form).
pub const PLANNED: &[&str] = &["postgres", "mysql", "sqlite", "loki", "influxdb", "opensearch-sql", "grafana-json", "files (iyakki-data)", "kubernetes-metrics", "http-json"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    #[test]
    fn every_kind_has_a_form_and_builds() {
        let up = FakeUp::new();
        let d = deps(&up, &[("uvault:svc.t", "tok")], Arc::new(FakeRunner::new(&[])));
        for k in kinds() {
            assert!(!k.fields.is_empty() && !k.label.is_empty());
            let mut c = SourceConfig { id: "x".into(), kind: k.kind.clone(), name: "n".into(), ..Default::default() };
            match k.kind.as_str() {
                "duckdb" => {
                    c.values.insert("files".into(), "/home/amy/a.csv".into());
                }
                _ => {
                    c.values.insert("url".into(), "http://h:1".into());
                }
            }
            let s = build(&c, &d).unwrap_or_else(|e| panic!("{}: {e}", k.kind));
            assert_eq!(s.kind(), k.kind);
        }
        assert!(build(&SourceConfig { id: "x".into(), kind: "nope".into(), name: "n".into(), ..Default::default() }, &d).is_err());
    }

    #[test]
    fn secrets_must_be_vault_references_and_never_values() {
        let info = super::kind("prometheus").unwrap();
        let mut c = fake::cfg("prometheus", "http://h:9090");
        c.values.insert("password".into(), "hunter2".into());
        c.secrets.insert("token".into(), "hunter3".into());
        let p = validate(&c, &info).join(" | ");
        assert!(p.contains("never stored in the settings") && p.contains("not a vault reference"), "{p}");
        assert!(!p.contains("hunter2") && !p.contains("hunter3"), "a rejected secret is not echoed: {p}");
        c.values.remove("password");
        c.secrets.insert("token".into(), "env:HOME".into());
        assert!(!validate(&c, &info).is_empty(), "env: is not the vault");
        c.secrets.insert("token".into(), "uvault:svc.prom".into());
        assert!(validate(&c, &info).is_empty());
        c.values.insert("url".into(), "https://user:pw@host".into());
        assert!(validate(&c, &info).join(" ").contains("do not put a user name"));
    }

    #[test]
    fn credentials_go_only_into_headers_and_debug_hides_them() {
        let up = FakeUp::new().on("buildinfo", 200, r#"{"status":"success","data":{"version":"2.0"}}"#);
        let d = deps(&up, &[("uvault:svc.prom", "S3CRET-TOKEN")], Arc::new(FakeRunner::new(&[])));
        let mut c = fake::cfg("prometheus", "http://h:9090");
        c.values.insert("auth".into(), "bearer".into());
        c.secrets.insert("token".into(), "uvault:svc.prom".into());
        let s = build(&c, &d).unwrap();
        s.test(&Cancel::new()).unwrap();
        let seen = up.seen.lock().unwrap().clone();
        assert!(seen.iter().all(|(_, url, _, body)| !url.contains("S3CRET") && !body.contains("S3CRET")));
        assert!(seen[0].2.iter().any(|(k, v)| k == "Authorization" && v == "Bearer S3CRET-TOKEN"));
        let http = Http::from_config(&c, &d, "x").unwrap();
        assert!(!format!("{http:?}").contains("S3CRET"));
        // a missing secret is a sentence, not a panic, and does not leak the reference's value
        c.secrets.insert("token".into(), "uvault:svc.gone".into());
        let e = build(&c, &d).err().unwrap();
        assert_eq!(e.kind, ErrKind::Config);
        assert!(e.fix.as_deref().unwrap_or("").contains("vault"));
    }

    #[test]
    fn basic_and_api_key_headers() {
        let up = FakeUp::new().on("/", 200, "{}");
        let d = deps(&up, &[("uvault:pw", "pass"), ("uvault:key", "KEY")], Arc::new(FakeRunner::new(&[])));
        let mut c = fake::cfg("elasticsearch", "http://h:9200");
        c.values.insert("auth".into(), "basic".into());
        c.values.insert("username".into(), "elastic".into());
        c.secrets.insert("password".into(), "uvault:pw".into());
        let h = Http::from_config(&c, &d, "x").unwrap();
        h.send("GET", "/", &[], &[], &Cancel::new()).unwrap();
        assert_eq!(up.seen.lock().unwrap()[0].2[0].1, "Basic ZWxhc3RpYzpwYXNz");
        c.values.insert("auth".into(), "api_key".into());
        c.secrets.insert("token".into(), "uvault:key".into());
        let h = Http::from_config(&c, &d, "x").unwrap();
        h.send("GET", "/", &[], &[], &Cancel::new()).unwrap();
        assert_eq!(up.seen.lock().unwrap()[1].2[0].1, "ApiKey KEY");
    }

    #[test]
    fn transport_options_are_passed_with_a_clamped_timeout_and_the_insecure_flag() {
        let up = FakeUp::new();
        let ft = Arc::new(FakeTransport(up.clone(), Default::default()));
        let mut d = deps(&up, &[], Arc::new(FakeRunner::new(&[])));
        d.transport = ft.clone();
        let mut c = fake::cfg("prometheus", "https://h");
        c.values.insert("timeout_s".into(), "9999".into());
        c.values.insert("insecure".into(), "true".into());
        c.values.insert("proxy".into(), "http://px:3128".into());
        build(&c, &d).unwrap();
        let o = ft.1.lock().unwrap()[0].clone();
        assert!(o.contains("timeout_s: 120") && o.contains("insecure: true") && o.contains("proxy: Some(\"set\")"), "{o}");
        assert!(!o.contains("px:3128"));
    }
}
