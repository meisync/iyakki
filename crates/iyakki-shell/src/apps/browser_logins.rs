//! Sign-ins the browser remembers for a person: kept in their personal vault (never in a plain file), filled in only on
//! the exact site they were saved for, and a generator for strong passwords.

use iyakki_web::doc::{Control, ControlKind, Doc};

use super::AppCx;

/// A saved sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cred {
    pub host: String,
    pub user: String,
    pub password: String,
}

/// The part of a site name that identifies it: lower case, without a leading `www.` and without a port.
pub fn host_key(host: &str) -> String {
    let h = host.trim().to_lowercase();
    let h = h.split(':').next().unwrap_or("").to_string();
    h.strip_prefix("www.").unwrap_or(&h).to_string()
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' }).collect()
}

fn prefix(host: &str) -> String {
    format!("web.{}.", clean(&host_key(host)).chars().take(40).collect::<String>())
}

/// The vault name for one sign-in: `web.<site>.<user>`, at most 64 characters.
pub fn secret_name(host: &str, user: &str) -> String {
    let mut n = format!("{}{}", prefix(host), clean(&user.to_lowercase()));
    n.truncate(64);
    n
}

fn encode(c: &Cred) -> String {
    serde_json::json!({ "host": host_key(&c.host), "user": c.user, "password": c.password }).to_string()
}

fn decode(text: &str) -> Option<Cred> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    Some(Cred { host: v["host"].as_str()?.to_string(), user: v["user"].as_str()?.to_string(), password: v["password"].as_str()?.to_string() })
}

/// Everything saved for exactly this site.
pub fn for_site(cx: &AppCx, host: &str) -> Result<Vec<Cred>, String> {
    let names = cx.services.user_names(&cx.session.user).map_err(|_| "Your personal vault is locked; sign in again to use saved passwords.".to_string())?;
    let want = prefix(host);
    let key = host_key(host);
    let mut out: Vec<Cred> = names
        .into_iter()
        .filter(|n| n.starts_with(&want))
        .filter_map(|n| cx.services.user_get(&cx.session.user, &n).ok().and_then(|v| decode(&v)))
        // the prefix is cut short for very long names, so check the real site again
        .filter(|c| c.host == key)
        .collect();
    out.sort_by(|a, b| a.user.cmp(&b.user));
    Ok(out)
}

/// Save (or replace) a sign-in. Returns false when exactly this one was already saved.
pub fn save(cx: &AppCx, c: &Cred) -> Result<bool, String> {
    if c.password.is_empty() || c.host.is_empty() {
        return Err("There is no password to save.".into());
    }
    let c = &Cred { host: host_key(&c.host), ..c.clone() };
    if for_site(cx, &c.host)?.iter().any(|o| o == c) {
        return Ok(false);
    }
    cx.services.user_set(&cx.session.user, &secret_name(&c.host, &c.user), &encode(c)).map_err(|e| e.to_string())?;
    Ok(true)
}

/// The user-name and password controls of the first sign-in form on a page: (the text field before the password, the
/// password field). A sign-up form with two password fields still gives the first.
pub fn login_controls(doc: &Doc) -> Option<(Option<usize>, usize)> {
    let pw = doc.controls.iter().position(|c| c.kind == ControlKind::Password && !c.disabled)?;
    let form = doc.controls[pw].form;
    let user = (0..pw).rev().find(|&i| {
        let c: &Control = &doc.controls[i];
        c.form == form && c.kind == ControlKind::Text && !c.disabled && !c.readonly
    });
    Some((user, pw))
}

/// A strong random password: 20 characters from letters and digits that cannot be mistaken for each other, with a symbol or
/// two, always including each kind.
pub fn generate_password() -> Result<String, String> {
    const LOWER: &[u8] = b"abcdefghijkmnopqrstuvwxyz";
    const UPPER: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    const DIGIT: &[u8] = b"23456789";
    const SYMBOL: &[u8] = b"-_.!#%+=";
    let all: Vec<u8> = [LOWER, UPPER, DIGIT, SYMBOL].concat();
    let mut raw = [0u8; 512];
    iyakki_sec::random_fill(&mut raw).map_err(|e| format!("no randomness: {e:?}"))?;
    // pick without bias: throw away bytes outside the largest multiple of the alphabet size
    let limit = 256 - 256 % all.len();
    let mut pick = raw.iter().filter(|b| (**b as usize) < limit).map(|b| all[*b as usize % all.len()]).collect::<Vec<u8>>().into_iter();
    let mut pw: Vec<u8> = Vec::new();
    // one of each kind first, then the rest, then mixed up
    for set in [LOWER, UPPER, DIGIT, SYMBOL] {
        pw.push(pick.find(|c| set.contains(c)).ok_or("no randomness")?);
    }
    while pw.len() < 20 {
        pw.push(pick.next().ok_or("no randomness")?);
    }
    let mut order = [0u8; 20];
    iyakki_sec::random_fill(&mut order).map_err(|e| format!("no randomness: {e:?}"))?;
    for i in (1..pw.len()).rev() {
        pw.swap(i, order[i] as usize % (i + 1));
    }
    String::from_utf8(pw).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;

    fn cx(tag: &str) -> AppCx {
        let cx = cx_with_admin_store(tag);
        cx.services.on_login("root", "rootpass12", true);
        cx
    }

    #[test]
    fn a_sign_in_is_saved_in_the_vault_found_only_for_its_own_site_and_not_saved_twice() {
        let cx = cx("login-save");
        let c = Cred { host: "www.Example.com".into(), user: "Alice@x.org".into(), password: "s3cret-pw".into() };
        assert_eq!(save(&cx, &c), Ok(true));
        assert_eq!(save(&cx, &c), Ok(false), "the same one again changes nothing");
        let found = for_site(&cx, "example.com:443").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].user.as_str(), found[0].password.as_str()), ("Alice@x.org", "s3cret-pw"));
        assert!(for_site(&cx, "evil-example.com").unwrap().is_empty(), "another site gets nothing");
        assert!(for_site(&cx, "example.com.evil.org").unwrap().is_empty(), "a longer name that starts the same gets nothing");
        assert!(for_site(&cx, "mail.example.com").unwrap().is_empty(), "a sub-site is another site");
        // a changed password for the same user replaces the old one
        let changed = Cred { password: "newer-pw-1".into(), ..c.clone() };
        assert_eq!(save(&cx, &changed), Ok(true));
        let found = for_site(&cx, "example.com").unwrap();
        assert_eq!((found.len(), found[0].password.as_str()), (1, "newer-pw-1"));
        // a second account on the same site sits beside it
        save(&cx, &Cred { user: "bob".into(), ..c }).unwrap();
        assert_eq!(for_site(&cx, "example.com").unwrap().len(), 2);
        // it lives in the vault, not in a plain file
        let names = cx.services.user_names("root").unwrap();
        assert!(names.iter().all(|n| n.starts_with("web.example_com.")), "{names:?}");
        assert!(!std::fs::read_dir(cx.services.dir()).unwrap().flatten().any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("newer-pw-1"))), "the password is in no readable file");
    }

    #[test]
    fn a_locked_vault_says_so_and_empty_passwords_are_not_saved() {
        let cx = cx_with_admin_store("login-locked"); // never signed in to the vault
        assert!(for_site(&cx, "a.org").unwrap_err().contains("locked"));
        assert!(save(&cx, &Cred { host: "a.org".into(), user: "u".into(), password: String::new() }).is_err());
    }

    #[test]
    fn the_sign_in_form_is_found_with_its_user_name_field() {
        let doc = iyakki_web::doc::from_html("<form><input name=q></form><form action=/in><input type=hidden name=t><input name=email><input type=password name=p><input type=password name=again><input type=submit></form>");
        let (user, pw) = login_controls(&doc).expect("a sign-in form");
        assert_eq!(doc.controls[user.unwrap()].name, "email", "the text field before the password, in the same form");
        assert_eq!(doc.controls[pw].name, "p");
        assert!(login_controls(&iyakki_web::doc::from_html("<form><input name=q></form>")).is_none());
    }

    #[test]
    fn generated_passwords_are_long_different_and_mix_every_kind_of_character() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let p = generate_password().unwrap();
            assert_eq!(p.len(), 20);
            assert!(p.chars().any(|c| c.is_ascii_lowercase()) && p.chars().any(|c| c.is_ascii_uppercase()) && p.chars().any(|c| c.is_ascii_digit()) && p.chars().any(|c| "-_.!#%+=".contains(c)), "{p}");
            assert!(!p.contains(['0', 'O', '1', 'l', 'I']), "no look-alikes: {p}");
            seen.insert(p);
        }
        assert_eq!(seen.len(), 50);
        assert_eq!(secret_name("a.org", "Bob Smith"), "web.a_org.bob_smith");
        assert!(secret_name(&"x".repeat(100), &"y".repeat(100)).len() <= 64);
    }
}
