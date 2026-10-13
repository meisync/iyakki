//! What sites keep in the browser between visits: cookies that outlive the session and each site's `localStorage`. They
//! hold sign-ins, so they are kept in the person's own vault (encrypted with their password), never in a plain file. The
//! vault is open only while the person is signed in; without it the browser works as before and keeps nothing.

use std::collections::BTreeMap;

use iyakki_web::cookies::Cookie;
use iyakki_js::Sites;

use super::AppCx;

const COOKIES: &str = "wsd.cookies";
/// A vault secret holds 64 KiB; leave room for the wrapper.
const MAX_BYTES: usize = 60 * 1024;

fn ls_name(host: &str) -> String {
    let clean: String = host.to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' { c } else { '_' }).collect();
    format!("wsd.ls.{clean}").chars().take(64).collect()
}

fn cookie_json(c: &Cookie) -> serde_json::Value {
    serde_json::json!({ "n": c.name, "v": c.value, "d": c.domain, "s": c.subdomains, "p": c.path, "e": c.expires, "x": c.secure, "h": c.http_only, "ss": c.same_site })
}

fn cookie_from(v: &serde_json::Value) -> Option<Cookie> {
    Some(Cookie {
        name: v["n"].as_str()?.to_string(),
        value: v["v"].as_str()?.to_string(),
        domain: v["d"].as_str()?.to_string(),
        subdomains: v["s"].as_bool().unwrap_or(false),
        path: v["p"].as_str().unwrap_or("/").to_string(),
        expires: v["e"].as_i64(),
        secure: v["x"].as_bool().unwrap_or(false),
        http_only: v["h"].as_bool().unwrap_or(false),
        same_site: v["ss"].as_str().map(str::to_string),
    })
}

/// Characters of an `indexedDB` snapshot per vault entry (JSON escaping can double them, a secret holds 64 KiB).
const IDB_CHUNK: usize = 24 * 1024;
/// Most chunks (so most data, about 1 MB) one site's databases may take in the vault.
const IDB_MAX_CHUNKS: usize = 40;

fn idb_name(host: &str, i: usize) -> String {
    let clean: String = host.to_lowercase().chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-' { c } else { '_' }).collect();
    let tail = format!(".{i}");
    let head: String = format!("wsd.idb.{clean}").chars().take(64 - tail.len()).collect();
    format!("{head}{tail}")
}

fn chunks_of(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(IDB_CHUNK).map(|c| c.iter().collect()).collect()
}

/// Read what was kept into the session's stores. Returns how many cookies and sites came back.
pub fn restore(cx: &AppCx, sites: &Sites, now: i64) -> (usize, usize) {
    let user = &cx.session.user;
    if !cx.services.user_vault_unlocked(user) {
        return (0, 0);
    }
    let mut cookies = 0;
    if let Ok(text) = cx.services.user_get(user, COOKIES) {
        if let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
            let saved: Vec<Cookie> = list.iter().filter_map(cookie_from).collect();
            cookies = saved.len();
            if let Ok(mut store) = sites.cookies.lock() {
                store.load(saved, now);
            }
        }
    }
    let mut n = 0;
    for name in cx.services.user_names(user).unwrap_or_default().into_iter().filter(|n| n.starts_with("wsd.ls.")) {
        let Ok(text) = cx.services.user_get(user, &name) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let (Some(origin), Some(map)) = (v["o"].as_str(), v["m"].as_object()) else { continue };
        let entries: BTreeMap<String, String> = map.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect();
        sites.restore_local(origin, entries);
        n += 1;
    }
    // indexedDB: pieces `{o, n, i, d}` put together per origin
    let mut parts: BTreeMap<String, Vec<(usize, usize, String)>> = BTreeMap::new();
    for name in cx.services.user_names(user).unwrap_or_default().into_iter().filter(|n| n.starts_with("wsd.idb.")) {
        let Ok(text) = cx.services.user_get(user, &name) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        if let (Some(o), Some(total), Some(i), Some(d)) = (v["o"].as_str(), v["n"].as_u64(), v["i"].as_u64(), v["d"].as_str()) {
            parts.entry(o.to_string()).or_default().push((total as usize, i as usize, d.to_string()));
        }
    }
    for (origin, mut v) in parts {
        v.sort_by_key(|p| p.1);
        let total = v[0].0;
        if v.len() == total && v.iter().enumerate().all(|(k, p)| p.1 == k && p.0 == total) {
            let whole: String = v.into_iter().map(|p| p.2).collect();
            if sites.restore_idb(&origin, &whole) {
                n += 1;
            }
        }
    }
    (cookies, n)
}

/// Write the cookies and the sites' storage to the vault. Quiet when there is no open vault. Sites whose storage is
/// too big for a vault entry are not kept (they start empty next time) rather than half-saved.
pub fn save(cx: &AppCx, sites: &Sites, now: i64, keep_cookies: bool) {
    let user = &cx.session.user;
    if !cx.services.user_vault_unlocked(user) || !keep_cookies {
        return;
    }
    let mut list: Vec<serde_json::Value> = sites.cookies.lock().map(|s| s.persistent(now).iter().map(cookie_json).collect()).unwrap_or_default();
    // the newest are last; drop the oldest until it fits
    let mut text = serde_json::to_string(&list).unwrap_or_default();
    while text.len() > MAX_BYTES && !list.is_empty() {
        let cut = (list.len() / 10).max(1);
        list.drain(..cut);
        text = serde_json::to_string(&list).unwrap_or_default();
    }
    let same = cx.services.user_get(user, COOKIES).map(|t| *t == text).unwrap_or(false);
    if !same {
        if list.is_empty() {
            let _ = cx.services.user_delete(user, COOKIES);
        } else {
            let _ = cx.services.user_set(user, COOKIES, &text);
        }
    }
    save_idb(cx, sites);
    for (origin, map) in sites.snapshot_local() {
        // only https sites on the usual port are kept
        let Some(host) = origin.strip_prefix("https://").and_then(|r| r.strip_suffix(":443")) else { continue };
        let body = serde_json::to_string(&serde_json::json!({ "o": origin, "m": map })).unwrap_or_default();
        if body.len() > MAX_BYTES {
            continue;
        }
        let name = ls_name(host);
        if cx.services.user_get(user, &name).map(|t| *t != body).unwrap_or(true) {
            let _ = cx.services.user_set(user, &name, &body);
        }
    }
}

fn save_idb(cx: &AppCx, sites: &Sites) {
    let user = &cx.session.user;
    for (origin, json) in sites.snapshot_idb() {
        let Some(host) = origin.strip_prefix("https://").and_then(|r| r.strip_suffix(":443")) else { continue };
        let pieces = chunks_of(&json);
        if pieces.len() > IDB_MAX_CHUNKS {
            continue;
        }
        for (i, d) in pieces.iter().enumerate() {
            let body = serde_json::to_string(&serde_json::json!({ "o": origin, "n": pieces.len(), "i": i, "d": d })).unwrap_or_default();
            let name = idb_name(host, i);
            if cx.services.user_get(user, &name).map(|t| *t != body).unwrap_or(true) {
                let _ = cx.services.user_set(user, &name, &body);
            }
        }
        // pieces left over from a longer earlier snapshot
        for i in pieces.len()..IDB_MAX_CHUNKS {
            if cx.services.user_get(user, &idb_name(host, i)).is_err() {
                break;
            }
            let _ = cx.services.user_delete(user, &idb_name(host, i));
        }
    }
}

/// Forget everything a host stored: cookies and `localStorage`, in the session and in the vault.
pub fn forget(cx: &AppCx, sites: &Sites, host: &str) {
    sites.forget_host(host);
    let user = &cx.session.user;
    let _ = cx.services.user_delete(user, &ls_name(host));
    for i in 0..IDB_MAX_CHUNKS {
        if cx.services.user_delete(user, &idb_name(host, i)).is_err() {
            break;
        }
    }
    if cx.services.user_vault_unlocked(user) {
        // rewrite the cookie list without this host's
        let now = iyakki_agent::clock::now_unix();
        save(cx, sites, now, true);
    }
}
