//! `iyakki vpn inspect FILE`: an **offline** report on a profile file. No root, no network, no device, nothing saved, nothing
//! connected. It exists so a person can ask "what is in my file, and would IYAKKI take it?" and paste the answer into a chat.
//!
//! # What is printed, and what never is
//!
//! Printed: the kind, whether a user name and password or a key passphrase are needed, **directive names** (as typed when they are
//! plain names, else `(setting)`) with counts and a verdict, how many inline blocks of each kind there are, the **names** of
//! ciphers, digests, TLS versions and compression settings (only when they are on the list of known algorithm names; anything else
//! is reported as "not a known algorithm name"), counts, sizes, and fixed sentences.
//!
//! Never printed: any value of a directive (servers, ports, addresses, routes, user names), the content of any block, any key, any
//! file name or path, and no part of an error message that carries a value (those are replaced by fixed sentences). The test
//! `nothing_from_the_input_reaches_the_output` plants canary strings everywhere and checks the output for them.
//!
//! Each line is judged by the **real importer** on a minimal profile that holds only that line, so the verdicts cannot drift from
//! what Import does.

use crate::error::VpnError;
use crate::net::{default_cluster_cidrs, Context};
use crate::{ovpn, text, wgconf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    OpenVpn,
    WireGuard,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Ignored(String),
    Refused { reason: String, advice: String },
}

impl Verdict {
    fn rank(&self) -> u8 {
        match self {
            Verdict::Accepted => 0,
            Verdict::Ignored(_) => 1,
            Verdict::Refused { .. } => 2,
        }
    }
    fn word(&self) -> &'static str {
        match self {
            Verdict::Accepted => "ACCEPTED",
            Verdict::Ignored(_) => "IGNORED",
            Verdict::Refused { .. } => "REFUSED",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need {
    Yes(String),
    No(String),
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub count: usize,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    pub name: String,
    pub count: usize,
    pub note: &'static str,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub kind: Detected,
    pub bytes: usize,
    pub lines: usize,
    pub credentials: Need,
    pub passphrase: Need,
    pub rows: Vec<Row>,
    pub blocks: Vec<BlockRow>,
    /// `(setting, algorithm name or a fixed phrase)`.
    pub crypto: Vec<(String, String)>,
    pub servers: usize,
    pub accepted: bool,
    /// Why the whole file is not accepted (a fixed sentence or a directive name with a fixed reason).
    pub problem: Option<String>,
    pub notes: Vec<String>,
    /// OpenVPN: the file itself asks for the default route. WireGuard: AllowedIPs contain a default route. `None` when unknown.
    pub full_by_file: Option<bool>,
    pub peers: usize,
}

const KNOWN_ALGORITHMS: &[&str] = &[
    "AES-128-GCM", "AES-192-GCM", "AES-256-GCM", "AES-128-CBC", "AES-192-CBC", "AES-256-CBC", "CHACHA20-POLY1305", "BF-CBC", "DES-EDE3-CBC", "CAMELLIA-128-CBC", "CAMELLIA-256-CBC", "SHA1", "SHA224", "SHA256", "SHA384",
    "SHA512", "MD5", "NONE", "TLS-ECDHE-RSA-WITH-AES-256-GCM-SHA384", "TLS-ECDHE-ECDSA-WITH-AES-256-GCM-SHA384", "TLS-DHE-RSA-WITH-AES-256-GCM-SHA384", "TLS-ECDHE-RSA-WITH-AES-128-GCM-SHA256",
    "TLS-ECDHE-ECDSA-WITH-AES-128-GCM-SHA256", "TLS-DHE-RSA-WITH-AES-128-GCM-SHA256", "TLS_AES_256_GCM_SHA384", "TLS_CHACHA20_POLY1305_SHA256", "TLS_AES_128_GCM_SHA256", "1.0", "1.1", "1.2", "1.3", "OR-HIGHEST",
    "LZO", "LZ4", "LZ4-V2", "STUB", "STUB-V2", "YES", "NO", "ADAPTIVE",
];

/// A value made of known algorithm names separated by `:` or `,` is shown; anything else is not.
fn algorithm_text(args: &[String]) -> String {
    let all: Vec<String> = args.iter().flat_map(|a| a.split([':', ',']).map(str::to_string).collect::<Vec<_>>()).collect();
    if !all.is_empty() && all.len() <= 12 && all.iter().all(|a| KNOWN_ALGORITHMS.contains(&a.to_ascii_uppercase().as_str())) {
        all.join(" ")
    } else {
        "a value that is not a known algorithm name (not shown)".to_string()
    }
}

fn canonical(k: &str) -> &'static str {
    match k {
        "privatekey" => "PrivateKey",
        "address" => "Address",
        "dns" => "DNS",
        "mtu" => "MTU",
        "listenport" => "ListenPort",
        "fwmark" => "FwMark",
        "preup" => "PreUp",
        "postup" => "PostUp",
        "predown" => "PreDown",
        "postdown" => "PostDown",
        "saveconfig" => "SaveConfig",
        "table" => "Table",
        "publickey" => "PublicKey",
        "presharedkey" => "PresharedKey",
        "endpoint" => "Endpoint",
        "allowedips" => "AllowedIPs",
        _ => "PersistentKeepalive",
    }
}

fn ctx() -> Context {
    Context { cluster: default_cluster_cidrs(), lan: vec![], tunnel: vec![] }
}

/// A name from the file, only when the importer knows it; anything else could be a pasted secret.
fn known_name(n: &str) -> &str {
    if ovpn::known_directive(n) || wgconf::KNOWN_KEYS.contains(&n.to_ascii_lowercase().as_str()) || matches!(n, "[Interface]" | "[Peer]" | "section" | "line" | "url") {
        n
    } else {
        "(unknown setting)"
    }
}

/// A sentence for an import error that carries no value.
fn safe(e: &VpnError) -> String {
    match e {
        VpnError::Rejected { line, directive, reason } => format!("line {line}: \"{}\" is not accepted: {reason}", known_name(directive)),
        VpnError::Invalid { line, directive, reason } => format!("line {line}: \"{}\" is not valid: {reason}", known_name(directive)),
        VpnError::Unsafe(_) => "an address, route or server in the file is not allowed here (loopback, link-local, the cluster network, the local network or a server that cannot be used)".to_string(),
        VpnError::Netlink(_) => "internal error".to_string(),
        other => other.to_string(),
    }
}

fn advice_for(reason: &str) -> String {
    if reason.contains("put the content inline") {
        "put the certificate or key text inside a <ca>, <cert> or <key> block, and remove the line that names the file".to_string()
    } else if reason.contains("VORACLE") {
        "remove this line; if the server insists on compression, ask the provider for a profile without it".to_string()
    } else if reason.contains("layer-3") || reason.contains("tun is supported") {
        "this profile needs a bridged (tap) tunnel, which is not supported; ask the provider for a tun profile".to_string()
    } else if reason.contains("none") && reason.contains("encryption") {
        "remove this line, or ask the provider for a profile that encrypts".to_string()
    } else {
        "remove this line (then check with your provider that the profile still works without it)".to_string()
    }
}

fn judge_err(e: &VpnError) -> Verdict {
    match e {
        VpnError::Rejected { reason, .. } => Verdict::Refused { reason: reason.clone(), advice: advice_for(reason) },
        VpnError::Invalid { reason, .. } => Verdict::Refused { reason: format!("the value is not accepted: {reason}"), advice: "correct or remove this line".to_string() },
        VpnError::Unsafe(_) => Verdict::Refused { reason: safe(e), advice: "remove this line, or correct the address".to_string() },
        other => Verdict::Refused { reason: safe(other), advice: "remove this line".to_string() },
    }
}

fn merge(rows: &mut Vec<Row>, name: String, v: Verdict) {
    match rows.iter_mut().find(|r| r.name == name) {
        Some(r) => {
            r.count += 1;
            if v.rank() > r.verdict.rank() {
                r.verdict = v;
            }
        }
        None => rows.push(Row { name, count: 1, verdict: v }),
    }
}

fn detect(src: &str) -> Detected {
    let mut ovpn_hits = 0;
    for l in src.lines() {
        let t = l.trim();
        if t.eq_ignore_ascii_case("[interface]") || t.eq_ignore_ascii_case("[peer]") {
            return Detected::WireGuard;
        }
        let first = t.split_whitespace().next().unwrap_or("");
        let first = first.strip_prefix("--").unwrap_or(first);
        if ["client", "remote", "dev", "proto", "tls-client", "ca", "<ca>", "auth-user-pass", "cipher", "persist-key", "nobind", "remote-cert-tls", "resolv-retry"].contains(&first) {
            ovpn_hits += 1;
        }
    }
    if ovpn_hits >= 2 {
        Detected::OpenVpn
    } else {
        Detected::Unknown
    }
}

/// Look at a file's bytes.
pub fn inspect(input: &[u8]) -> Report {
    let mut r = Report {
        kind: Detected::Unknown,
        bytes: input.len(),
        lines: input.iter().filter(|&&b| b == b'\n').count() + usize::from(input.last().is_some_and(|&b| b != b'\n')),
        credentials: Need::Unknown("not checked".into()),
        passphrase: Need::Unknown("not checked".into()),
        rows: vec![],
        blocks: vec![],
        crypto: vec![],
        servers: 0,
        accepted: false,
        problem: None,
        notes: vec![],
        full_by_file: None,
        peers: 0,
    };
    let src = match text::clean(input) {
        Ok(s) => s,
        Err(e) => {
            r.problem = Some(safe(&e));
            return r;
        }
    };
    r.kind = detect(&src);
    match r.kind {
        Detected::OpenVpn => inspect_ovpn(&src, input, &mut r),
        Detected::WireGuard => inspect_wg(&src, input, &mut r),
        Detected::Unknown => r.problem = Some("this does not look like an OpenVPN (.ovpn) or WireGuard (.conf) profile".to_string()),
    }
    r
}

fn inspect_ovpn(src: &str, input: &[u8], r: &mut Report) {
    const BASE: &str = "remote vpn.example.com 1194\n<ca>\nx\n</ca>\n";
    let mut in_block: Option<(String, bool)> = None; // (name, contains "ENCRYPTED")
    let mut cred_plain = false;
    let mut cred_file = false;
    let mut askpass = false;
    let mut enc_key = false;
    let mut pkcs12 = false;
    let mut redirect = false;
    for raw in src.split('\n') {
        let line = raw.trim();
        if let Some((name, enc)) = in_block.as_mut() {
            if line == format!("</{name}>") {
                if (name == "key") && *enc {
                    enc_key = true;
                }
                in_block = None;
            } else if line.contains("ENCRYPTED") {
                *enc = true;
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(b) = line.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
            let (label, note): (String, &'static str) = match b {
                "ca" | "cert" | "dh" | "extra-certs" => (b.to_string(), "kept in the profile (public)"),
                "key" | "tls-auth" | "tls-crypt" | "tls-crypt-v2" | "pkcs12" => (b.to_string(), "secret: would be moved to the vault"),
                _ => ("(other)".to_string(), "REFUSED: this block is not supported"),
            };
            if b == "pkcs12" {
                pkcs12 = true;
            }
            match r.blocks.iter_mut().find(|x| x.name == label) {
                Some(x) => x.count += 1,
                None => r.blocks.push(BlockRow { name: label, count: 1, note }),
            }
            in_block = Some((b.to_string(), false));
            continue;
        }
        let Ok(toks) = ovpn::tokenize(line) else {
            merge(&mut r.rows, "(a line that cannot be read)".into(), Verdict::Refused { reason: "quotes or backslashes that are not accepted".into(), advice: "remove this line".into() });
            continue;
        };
        let Some(first) = toks.first() else { continue };
        let dname = first.strip_prefix("--").unwrap_or(first);
        let args = &toks[1..];
        let shown_name = if ovpn::known_directive(dname) { dname.to_string() } else { "(unknown directive)".to_string() };
        // facts, taken from the line and never printed as values
        match dname {
            "auth-user-pass" => {
                if args.is_empty() {
                    cred_plain = true
                } else {
                    cred_file = true
                }
            }
            "askpass" => askpass = true,
            "redirect-gateway" => redirect = true,
            "remote" => r.servers += 1,
            "cipher" | "data-ciphers" | "data-ciphers-fallback" | "ncp-ciphers" | "auth" | "tls-cipher" | "tls-ciphersuites" | "tls-version-min" | "tls-version-max" | "compress" | "comp-lzo" | "allow-compression" => {
                let v = if args.is_empty() { "(no value)".to_string() } else { algorithm_text(args) };
                r.crypto.push((shown_name.clone(), v));
            }
            "route" | "route-ipv6" if args.first().is_some_and(|a| a == "0.0.0.0" || a.starts_with("::/0")) && args.get(1).is_none_or(|m| m == "0.0.0.0") => redirect = true,
            _ => {}
        }
        // the verdict: the real importer on a profile of just this line
        let synthetic = format!("{BASE}{line}\n");
        let v = match ovpn::parse("check", synthetic.as_bytes(), &ctx()) {
            Ok(imp) => {
                if imp.warnings.iter().any(|w| w.contains("ignored")) {
                    let why = if dname == "dhcp-option" {
                        "only DNS options are used; this one is not".to_string()
                    } else if matches!(dname, "block-outside-dns" | "register-dns") {
                        "Windows only".to_string()
                    } else {
                        "harmless on these images and not used".to_string()
                    };
                    Verdict::Ignored(why)
                } else {
                    Verdict::Accepted
                }
            }
            Err(e) => judge_err(&e),
        };
        merge(&mut r.rows, shown_name, v);
    }
    if let Some((name, _)) = in_block {
        let _ = name;
        r.notes.push("a block is not closed at the end of the file".to_string());
    }
    r.credentials = if cred_plain {
        Need::Yes("auth-user-pass without a file: IYAKKI will ask for a user name and password when you connect".into())
    } else if cred_file {
        Need::Yes("auth-user-pass names a file: that file is not read; IYAKKI asks for the user name and password instead".into())
    } else {
        Need::No("the file does not ask for a user name and password (a server can still require a certificate)".into())
    };
    r.passphrase = if enc_key || askpass {
        Need::Yes(if enc_key { "the private key in the file is encrypted".into() } else { "askpass is present".into() })
    } else if pkcs12 {
        Need::Unknown("a pkcs12 bundle is present; it may be protected by a passphrase that cannot be seen from outside".into())
    } else {
        Need::No("no encrypted private key was found".into())
    };
    r.full_by_file = Some(redirect);
    match ovpn::parse("check", input, &ctx()) {
        Ok(_) => r.accepted = true,
        Err(e) => r.problem = Some(safe(&e)),
    }
}

fn inspect_wg(src: &str, input: &[u8], r: &mut Report) {
    const K1: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const K2: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
    let template = |skip: &str, iface: &str, peer: &str| {
        let mut s = String::from("[Interface]\n");
        if skip != "privatekey" {
            s.push_str(&format!("PrivateKey = {K1}\n"));
        }
        if skip != "address" {
            s.push_str("Address = 10.8.0.2/24\n");
        }
        s.push_str(iface);
        s.push_str("\n[Peer]\n");
        if skip != "publickey" {
            s.push_str(&format!("PublicKey = {K2}\n"));
        }
        if skip != "allowedips" {
            s.push_str("AllowedIPs = 10.8.0.9/32\n");
        }
        s.push_str(peer);
        s.push('\n');
        s
    };
    let mut section = 0u8; // 1 interface, 2 peer
    let mut default_route = false;
    let mut any_allowed = false;
    let mut has_private = false;
    for raw in src.split('\n') {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            match line.to_ascii_lowercase().as_str() {
                "[interface]" => section = 1,
                "[peer]" => {
                    section = 2;
                    r.peers += 1;
                }
                _ => {
                    section = 0;
                    merge(&mut r.rows, "(unknown section)".into(), Verdict::Refused { reason: "only [Interface] and [Peer] exist".into(), advice: "remove this section".into() });
                }
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            merge(&mut r.rows, "(a line without =)".into(), Verdict::Refused { reason: "expected Key = Value".into(), advice: "remove this line".into() });
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let shown_name = format!("{} {}", if section == 2 { "[Peer]" } else { "[Interface]" }, if wgconf::KNOWN_KEYS.contains(&key.as_str()) { canonical(&key) } else { "(unknown setting)" });
        if section == 0 {
            merge(&mut r.rows, "(a setting before any section)".into(), Verdict::Refused { reason: "a setting before any section".into(), advice: "move it under [Interface] or [Peer]".into() });
            continue;
        }
        if key == "privatekey" && section == 1 {
            has_private = true;
        }
        if key == "allowedips" && section == 2 {
            any_allowed = true;
            if v.split(',').any(|c| matches!(c.trim(), "0.0.0.0/0" | "::/0")) {
                default_route = true;
            }
        }
        if key == "presharedkey" && section == 2 {
            r.notes.push("a peer has a preshared key".to_string());
        }
        let line_t = format!("{} = {}", k.trim(), v.trim());
        let synthetic = if section == 1 { template(&key, &line_t, "") } else { template(&key, "", &line_t) };
        let verdict = match wgconf::parse("check", synthetic.as_bytes(), &ctx()) {
            Ok(imp) => {
                if imp.warnings.iter().any(|w| w.contains("ignored")) {
                    Verdict::Ignored("only addresses are used for DNS".into())
                } else {
                    Verdict::Accepted
                }
            }
            Err(e) => judge_err(&e),
        };
        merge(&mut r.rows, shown_name, verdict);
    }
    r.notes.dedup();
    r.credentials = Need::No("WireGuard uses keys, not a user name and password".into());
    r.passphrase = Need::No(if has_private { "the private key is a plain key (WireGuard keys have no passphrase)".into() } else { "no private key was found".into() });
    r.crypto.push(("encryption".into(), "fixed by WireGuard (Curve25519, ChaCha20-Poly1305, BLAKE2s); nothing to choose".into()));
    r.crypto.push(("compression".into(), "none (WireGuard has none)".into()));
    r.full_by_file = if any_allowed { Some(default_route) } else { None };
    r.servers = 0;
    match wgconf::parse("check", input, &ctx()) {
        Ok(imp) => {
            r.accepted = true;
            if let crate::profile::Body::Wireguard(w) = &imp.body {
                r.servers = w.peers.iter().filter(|p| p.endpoint.is_some()).count();
            }
        }
        Err(e) => r.problem = Some(safe(&e)),
    }
}

impl Report {
    /// The report as text. See the module comment for what it never contains.
    pub fn render(&self) -> String {
        let mut o = String::new();
        let kind = match self.kind {
            Detected::OpenVpn => "OpenVPN profile (.ovpn)",
            Detected::WireGuard => "WireGuard profile (.conf)",
            Detected::Unknown => "not recognised",
        };
        o.push_str("IYAKKI VPN profile check\n");
        o.push_str("Offline: nothing was connected, saved or sent. No value, key or file content is printed, so this is safe to paste.\n\n");
        o.push_str(&format!("1. Kind: {kind}\n   Size: {} bytes, {} lines\n\n", self.bytes, self.lines));
        let need = |n: &Need| match n {
            Need::Yes(s) => format!("YES - {s}"),
            Need::No(s) => format!("NO - {s}"),
            Need::Unknown(s) => format!("UNKNOWN - {s}"),
        };
        if self.kind != Detected::Unknown {
            o.push_str(&format!("2. Needs a user name and password: {}\n   Needs a private-key passphrase: {}\n\n", need(&self.credentials), need(&self.passphrase)));
            o.push_str("3. Settings in the file (names only):\n");
            let w = self.rows.iter().map(|r| r.name.chars().count()).max().unwrap_or(4).max(4);
            o.push_str(&format!("   {:<w$}  {:>5}  {:<8}  NOTE\n", "NAME", "LINES", "VERDICT", w = w));
            for r in &self.rows {
                let note = match &r.verdict {
                    Verdict::Accepted => String::new(),
                    Verdict::Ignored(why) => why.clone(),
                    Verdict::Refused { reason, advice } => format!("{reason}. What you can do: {advice}."),
                };
                o.push_str(format!("   {:<w$}  {:>5}  {:<8}  {}", r.name, r.count, r.verdict.word(), note, w = w).trim_end());
                o.push('\n');
            }
            if self.rows.is_empty() {
                o.push_str("   (no settings found)\n");
            }
            o.push('\n');
            if self.kind == Detected::OpenVpn {
                o.push_str("   Inline blocks (counted, never printed):\n");
                for b in &self.blocks {
                    o.push_str(&format!("   {:<14} {:>3}  {}\n", b.name, b.count, b.note));
                }
                if self.blocks.is_empty() {
                    o.push_str("   (none)\n");
                }
                o.push('\n');
            }
        }
        if self.kind != Detected::Unknown {
            o.push_str("   Encryption and compression, by name:\n");
            for (k, v) in &self.crypto {
                o.push_str(&format!("   {k}: {v}\n"));
            }
            if self.crypto.is_empty() {
                o.push_str("   (the file sets none; the server decides)\n");
            }
            o.push('\n');
        }
        o.push_str("4. Would IYAKKI accept this profile as it is: ");
        if self.accepted {
            o.push_str("YES\n");
        } else {
            o.push_str("NO\n");
            if let Some(p) = &self.problem {
                o.push_str(&format!("   First problem: {p}\n"));
            }
            let refused: Vec<&Row> = self.rows.iter().filter(|r| matches!(r.verdict, Verdict::Refused { .. })).collect();
            if !refused.is_empty() {
                o.push_str(&format!("   {} setting(s) are REFUSED (see the table); fix them and run the check again.\n", refused.len()));
            }
        }
        for n in &self.notes {
            o.push_str(&format!("   Note: {n}\n"));
        }
        if self.kind != Detected::Unknown {
            o.push_str("\n   What the two modes would do with it (planned behaviour; the connector is not finished):\n");
            match (self.kind, self.full_by_file) {
                (Detected::WireGuard, Some(true)) => o.push_str("   FULL:   all traffic goes through the VPN (the file's AllowedIPs cover everything).\n"),
                (Detected::WireGuard, Some(false)) => o.push_str("   FULL:   only the networks in the file's AllowedIPs go through the VPN; the rest goes direct.\n"),
                (Detected::OpenVpn, Some(true)) => o.push_str("   FULL:   all traffic goes through the VPN (the file asks for the default route).\n"),
                (Detected::OpenVpn, _) => o.push_str("   FULL:   all traffic goes through the VPN if the server pushes the default route; the file itself does not ask for it.\n"),
                _ => o.push_str("   FULL:   all traffic goes through the VPN as far as the file allows.\n"),
            }
            o.push_str("   HYBRID: only what your filters match (names, URLs, address ranges) goes through the VPN; everything else goes direct, and your system DNS settings are not replaced.\n");
            if self.kind == Detected::WireGuard {
                match self.full_by_file {
                    Some(true) => o.push_str("           Any filter works, because the file's AllowedIPs cover everything.\n"),
                    Some(false) => o.push_str("           A filter only works for destinations inside the file's AllowedIPs; others cannot be sent through this tunnel.\n"),
                    None => {}
                }
            }
            o.push_str("   LAN, the cluster and developer mode keep working in both modes.\n");
            o.push_str(&format!("   Servers in the file: {}\n", self.servers));
            if self.kind == Detected::WireGuard {
                o.push_str(&format!("   Peers in the file: {}\n", self.peers));
            }
        }
        o
    }
}

// ---- the command --------------------------------------------------------------------------------------------------------

const USAGE: &str = "usage: iyakki vpn inspect FILE   (reads a local .ovpn or WireGuard .conf; offline; prints names and counts, never values)";

/// `iyakki vpn ...`. Returns the exit code: 0 the file would be accepted, 1 it would not, 2 usage or read error.
pub fn cli_main(args: &[String]) -> i32 {
    let (code, out, err) = run(args, &|p| {
        use std::io::Read;
        let f = std::fs::File::open(p).map_err(|e| e.kind().to_string())?;
        let mut buf = Vec::new();
        f.take(text::MAX_BYTES as u64 + 1).read_to_end(&mut buf).map_err(|e| e.kind().to_string())?;
        Ok(buf)
    });
    if !out.is_empty() {
        print!("{out}");
    }
    if !err.is_empty() {
        eprintln!("{err}");
    }
    code
}

/// The testable core of [`cli_main`]: `(exit code, stdout, stderr)`. `read` returns the file's bytes (at most the size limit plus
/// one) or a short reason; the reason never contains the path.
pub fn run(args: &[String], read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> (i32, String, String) {
    match args {
        [cmd, file] if cmd == "inspect" => match read(file) {
            Ok(bytes) => {
                let r = inspect(&bytes);
                (if r.accepted { 0 } else { 1 }, r.render(), String::new())
            }
            Err(why) => (2, String::new(), format!("cannot read the file ({why})")),
        },
        _ => (2, String::new(), USAGE.to_string()),
    }
}
