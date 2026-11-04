use super::*;

fn with(f: impl FnOnce(&mut TenantConfig)) -> TenantConfig {
    let mut c = TenantConfig::example("acme");
    f(&mut c);
    c
}

#[test]
fn the_example_is_valid_and_round_trips_through_json() {
    let c = TenantConfig::example("acme");
    assert!(c.check().is_ok());
    let text = serde_json::to_string(&c).unwrap();
    assert_eq!(TenantConfig::parse(&text).unwrap(), c);
    // only the tenant is needed: everything else has a default
    let min = TenantConfig::parse(r#"{"tenant":"acme"}"#).unwrap();
    assert_eq!(min.namespace(), "iyakki-acme");
    assert_eq!(min.members[0].role, "admin");
    assert!(TenantConfig::parse(r#"{"tenant":"acme","surprise":1}"#).is_err(), "unknown keys are refused");
}

#[test]
fn nothing_but_plain_names_reaches_the_yaml() {
    for bad in ["", "Acme", "1acme", "acme-", "a b", "acme\nkind: Secret", "acme:x", &"a".repeat(41)] {
        assert!(with(|c| c.tenant = bad.into()).check().is_err(), "tenant {bad:?}");
    }
    assert!(with(|c| c.domain = Some("nodot".into())).check().is_err());
    assert!(with(|c| c.domain = Some("a.b\n  evil: 1".into())).check().is_err());
    assert!(with(|c| c.image = "iyakki:1\" ; x".into()).check().is_err());
    assert!(with(|c| c.storage = "2Gi\nx".into()).check().is_err());
    assert!(with(|c| c.storage = "lots".into()).check().is_err());
    assert!(with(|c| c.cpu_limit = "500m".into()).check().is_ok());
    assert!(with(|c| c.cpu_limit = "2.5".into()).check().is_ok());
    assert!(with(|c| c.cpu_limit = "m".into()).check().is_err());
    assert!(with(|c| c.members = vec![Member { name: "bob".into(), role: "root".into() }]).check().is_err(), "no such role");
    assert!(with(|c| c.members = vec![Member { name: "bob".into(), role: "user".into() }]).check().is_err(), "needs an administrator");
    assert!(with(|c| c.members.push(c.members[0].clone())).check().is_err(), "twice");
    assert!(with(|c| c.members = vec![]).check().is_err());
}

#[test]
fn the_manifests_hold_the_tenants_objects_and_no_password() {
    let y = manifests(&with(|c| c.members.push(Member { name: "mia".into(), role: "user".into() }))).unwrap();
    for k in ["kind: Namespace", "kind: ResourceQuota", "kind: PersistentVolumeClaim", "kind: Deployment", "kind: Service", "kind: NetworkPolicy"] {
        assert!(y.contains(k), "{k}");
    }
    assert!(!y.contains("kind: Ingress"), "no domain, no ingress");
    assert!(y.contains("name: iyakki-acme") && y.contains("namespace: iyakki-acme"));
    assert!(y.contains("account-admin") && y.contains("account-mia"), "an init container per member");
    assert!(y.contains("\"--role\", \"user\"") && y.contains("/secret/password-mia"));
    assert!(y.contains("secretName: iyakki-accounts"));
    assert!(y.contains("IYAKKI_WEB") && y.contains("value: \"1\""), "no domain: the pod does its own HTTPS");
    assert!(y.contains("runAsNonRoot: true") && y.contains("readOnlyRootFilesystem: true") && y.contains("drop: [\"ALL\"]"));
    assert!(y.contains("replicas: 1"));
    assert!(!y.contains("--from-literal=password-admin='<") || y.contains("# "), "the secret command is only in a comment");
    // every document is valid YAML-ish: the separators split it into the same number of objects as kinds
    assert_eq!(y.matches("\n---\n").count() + 1, y.matches("\nkind: ").count() + y.starts_with("kind: ") as usize);
}

#[test]
fn a_domain_makes_an_ingress_for_http_and_websocket() {
    let y = manifests(&with(|c| {
        c.domain = Some("acme.example.org".into());
        c.tls_secret = Some("acme-tls".into());
    }))
    .unwrap();
    assert!(y.contains("kind: Ingress") && y.contains("host: acme.example.org"));
    assert!(y.contains("secretName: acme-tls") && y.contains("hosts: [\"acme.example.org\"]"));
    assert!(y.contains("proxy-read-timeout"), "long WebSocket sessions are not cut");
    assert!(y.contains("value: \"http:8080\"") && y.contains("containerPort: 8080"), "the ingress ends the TLS: the pod speaks plain HTTP");
}

#[test]
fn the_cloud_profile_needs_its_certificate_and_checks_its_own_service_file() {
    assert!(with(|c| c.profile = Profile::Cloud).check().is_err(), "no certificate Secret");
    let c = with(|c| {
        c.profile = Profile::Cloud;
        c.tls_secret = Some("acme-tls".into());
        c.domain = Some("acme.example.org".into());
    });
    let y = manifests(&c).unwrap();
    assert!(y.contains("kind: ConfigMap") && y.contains("service.json: |"));
    assert!(y.contains("--profile") && y.contains("containerPort: 8444") && y.contains("scheme: HTTPS"));
    assert!(y.contains("secretName: acme-tls") && y.contains("/etc/iyakki/tls"));
    let json = service_json(&c).unwrap();
    assert!(json.contains("https://acme.example.org"));
    assert!(service_json(&with(|c| {
        c.profile = Profile::Cloud;
        c.tls_secret = Some("t".into());
        c.allow_from = vec!["0.0.0.0/0".into()];
    }))
    .is_err());
}

#[test]
fn the_secret_command_names_each_member_and_carries_no_value() {
    let c = with(|c| c.members.push(Member { name: "mia".into(), role: "user".into() }));
    let s = secret_command(&c);
    assert!(s.contains("-n iyakki-acme") && s.contains("password-admin=") && s.contains("password-mia="));
    assert!(s.contains("<PASSWORD of admin>"));
}

#[test]
fn memory_for_the_quota_is_three_times_the_limit() {
    assert_eq!(mem_times("1Gi", 3), "3Gi");
    assert_eq!(mem_times("512Mi", 3), "1536Mi");
}

#[test]
fn the_common_vault_is_mounted_read_only_only_when_asked() {
    let off = manifests(&TenantConfig::example("acme")).unwrap();
    assert!(!off.contains("iyakki-common"));
    let on = manifests(&with(|c| c.common_vault = true)).unwrap();
    assert!(on.contains("secretName: iyakki-common") && on.contains("mountPath: /etc/iyakki/common") && on.contains("readOnly: true"));
    assert!(on.contains("defaultMode: 0400"));
}

#[test]
fn isolation_defaults_to_folders_and_volumes_is_refused_until_it_is_built() {
    let c = with(|_| {});
    assert_eq!(c.isolation, crate::cloudstorage::Isolation::Folders);
    let e = TenantConfig::parse(r#"{"tenant":"acme","isolation":"volumes"}"#).unwrap_err();
    assert!(e.contains("not built yet"), "{e}");
    assert!(TenantConfig::parse(r#"{"tenant":"acme","isolation":"folders"}"#).is_ok());
}
