//! Paying for an app without IYAKKI ever touching money (F-APPSTORE-13). The Buy button opens the provider's own
//! checkout page in the browser tile; the provider (or the publisher's server) hands back a signed licence token that
//! [`super::license`] checks offline. This module is the two ends of that hand-over:
//!
//! * [`checkout_url`] builds the address to open: only the publisher's `purchase_url` over `https`, plus the app name and
//!   this device's random licence id so the token can be made for this device. Nothing else leaves the device: no
//!   account name, no hardware id, no card data (IYAKKI never has any).
//! * [`extract_token`] takes whatever the person was given (the token text itself, a file's text, a link with the token
//!   in its `#iyakki-licence=` part, an `iyakki-licence:` line from a QR code or an email) and returns the token JSON,
//!   which `Store::add_license` then verifies and keeps. A refund is the other direction: the serial goes on the signed
//!   revocation list and the app is locked again (tested below with the real verification path).
//!
//! The token's trust never comes from how it arrived: only the signature, the issuer allow-list, the device, the
//! account, the expiry and the revocation list decide (`Token::verify`).

pub use iyakki_shared::store_licencelink::{extract_token, token_in_address, MARK, PENDING_APP, PENDING_KEY};
use super::manifest::Pricing;

fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The address the Buy button opens in the browser tile.
pub fn checkout_url(pricing: &Pricing, app: &str, device_id: &str) -> Result<String, String> {
    if !pricing.model.needs_licence() {
        return Err("this app is not sold: there is nothing to buy".into());
    }
    let base = pricing.purchase_url.trim();
    if base.is_empty() {
        return Err("the publisher has not given a checkout address".into());
    }
    // Money only over https: an http address is refused even though the catalog may carry one for a free app's page.
    let rest = base.strip_prefix("https://").ok_or("the checkout address must start with https://")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') || host.contains(' ') {
        return Err("the checkout address has no usable host".into());
    }
    if device_id.len() != 32 || !device_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("this device has no licence id yet".into());
    }
    if !crate::skills::valid_skill_name(app) {
        return Err("the app name is not valid".into());
    }
    let base = base.split('#').next().unwrap_or(base);
    let sep = if base.contains('?') { '&' } else { '?' };
    let mut u = format!("{base}{sep}iyakki_app={}&iyakki_device={}", pct(app), pct(device_id));
    if !pricing.checkout_id.is_empty() {
        u.push_str(&format!("&checkout={}", pct(&pricing.checkout_id)));
    }
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::super::install::tests::{fx, NOW};
    use super::super::license::{LicenseError, Token, MAX_TOKEN_BYTES};
    use super::super::manifest::PriceModel;
    use super::*;
    use crate::pkg::{new_signing_key, public_key_hex};

    fn paid(url: &str) -> Pricing {
        Pricing { model: PriceModel::Paid, price: 499, currency: "USD".into(), purchase_url: url.into(), checkout_id: "co 1".into(), trial_days: 0 }
    }
    const DEV: &str = "0123456789abcdef0123456789abcdef";

    fn b64(b: &[u8]) -> String {
        crate::story::base64_encode(b).replace('+', "-").replace('/', "_").trim_end_matches('=').to_string()
    }

    #[test]
    fn a_return_page_address_hands_over_a_token_only_from_https_or_the_licence_scheme_and_only_from_the_fragment_or_query() {
        let tok = r#"{"app":"pro-editor","serial":7}"#;
        let code = b64(tok.as_bytes());
        assert_eq!(token_in_address(&format!("https://pay.example.org/thanks#{MARK}={code}")).as_deref(), Some(tok));
        assert_eq!(token_in_address(&format!("https://pay.example.org/thanks?order=1&{MARK}={code}")).as_deref(), Some(tok));
        assert_eq!(token_in_address(&format!("{MARK}:{code}")).as_deref(), Some(tok));
        // plain http, a token in the path, an unrelated address and a damaged code give nothing
        assert!(token_in_address(&format!("http://pay.example.org/thanks#{MARK}={code}")).is_none());
        assert!(token_in_address(&format!("https://pay.example.org/{MARK}={code}")).is_none());
        assert!(token_in_address("https://pay.example.org/thanks").is_none());
        assert!(token_in_address(&format!("https://pay.example.org/#{MARK}=!!!")).is_none());
        assert!(token_in_address(&format!("https://pay.example.org/#{MARK}={}", "A".repeat(100_000))).is_none());
    }

    #[test]
    fn the_checkout_address_carries_only_the_app_and_the_device_id() {
        let u = checkout_url(&paid("https://pay.example.org/buy?plan=a#top"), "pro-editor", DEV).unwrap();
        assert_eq!(u, format!("https://pay.example.org/buy?plan=a&iyakki_app=pro-editor&iyakki_device={DEV}&checkout=co%201"));
        let u = checkout_url(&paid("https://pay.example.org/"), "pro-editor", DEV).unwrap();
        assert!(u.starts_with("https://pay.example.org/?iyakki_app="));
        // nothing about the person
        assert!(!u.contains("account") && !u.contains("user"));
    }

    #[test]
    fn money_pages_must_be_https_and_the_inputs_valid() {
        assert!(checkout_url(&paid("http://pay.example.org/"), "a-b", DEV).unwrap_err().contains("https"));
        assert!(checkout_url(&paid(""), "a-b", DEV).is_err());
        assert!(checkout_url(&paid("https://user@evil.example/"), "a-b", DEV).is_err());
        assert!(checkout_url(&paid("https:///x"), "a-b", DEV).is_err());
        assert!(checkout_url(&paid("https://p.example/"), "../x", DEV).is_err());
        assert!(checkout_url(&paid("https://p.example/"), "a-b", "short").is_err());
        let free = Pricing { model: PriceModel::Free, ..paid("https://p.example/") };
        assert!(checkout_url(&free, "a-b", DEV).is_err());
        let sub = Pricing { model: PriceModel::Subscription, ..paid("https://p.example/") };
        assert!(checkout_url(&sub, "a-b", DEV).is_ok());
    }

    #[test]
    fn every_way_a_token_arrives_gives_the_same_json() {
        let k = new_signing_key().unwrap();
        let tok = Token::issue(&k, "pro-editor", "acme", DEV, "", "L-1", "standard", NOW, 0);
        let json = tok.to_json();
        let e = b64(json.as_bytes());
        assert_eq!(extract_token(&json).unwrap(), json.trim());
        assert_eq!(extract_token(&format!("  {json}\n")).unwrap(), json.trim());
        assert_eq!(extract_token(&format!("{MARK}:{e}")).unwrap(), json);
        assert_eq!(extract_token(&format!("https://shop.example/thanks#{MARK}={e}")).unwrap(), json);
        assert_eq!(extract_token(&format!("https://shop.example/thanks?order=7&{MARK}={e}")).unwrap(), json);
        // standard base64 with padding works too
        assert_eq!(extract_token(&format!("{MARK}:{}", crate::story::base64_encode(json.as_bytes()))).unwrap(), json);
        for bad in ["", "hello", "https://shop.example/thanks", &format!("{MARK}:!!!"), &format!("{MARK}:{}", b64(b"not json")), &"x".repeat(MAX_TOKEN_BYTES * 3)] {
            assert!(extract_token(bad).is_err(), "{bad}");
        }
        assert_eq!(Token::parse(&extract_token(&format!("{MARK}:{e}")).unwrap()).unwrap(), tok);
    }

    /// The whole path with the real verification: buy, receive by link, the app unlocks, a refund locks it again.
    #[test]
    fn receive_then_refund_locks_the_app_again() {
        let f = fx("checkout");
        let publisher = new_signing_key().unwrap();
        let signer = vec![public_key_hex(&publisher)];
        let a = super::super::install::tests::user("alice");
        let dev = f.store.licenses().device_id();
        let url = checkout_url(&paid("https://pay.example.org/buy"), "pro-editor", &dev).unwrap();
        assert!(url.contains(&dev));
        let tok = Token::issue(&publisher, "pro-editor", "acme", &dev, "", "L-77", "standard", NOW - 5, 0);
        let link = format!("https://pay.example.org/done#{MARK}={}", b64(tok.to_json().as_bytes()));
        let got = f.store.add_license(&a, &extract_token(&link).unwrap(), &|_| signer.clone()).unwrap();
        assert_eq!(got.serial, "L-77");
        assert!(f.store.licenses().get("pro-editor").is_some());
        // a token made for another device is refused even when it arrives the right way
        let other = Token::issue(&publisher, "pro-editor", "acme", "ffffffffffffffffffffffffffffffff", "", "L-78", "", NOW, 0);
        let l2 = format!("{MARK}:{}", b64(other.to_json().as_bytes()));
        assert!(f.store.add_license(&a, &extract_token(&l2).unwrap(), &|_| signer.clone()).unwrap_err().contains("different device"));
        // refund: the signed revocation list names the serial
        let list = super::super::revocation::sign_list(&f.root, 5, NOW, NOW + 86_400, vec![], vec![], vec![], vec!["L-77".into()]);
        let mut revs = f.store.revocations();
        assert!(revs.accept(&list, &f.store.roots, NOW).unwrap());
        let stored = f.store.licenses().get("pro-editor").unwrap();
        let check = super::super::license::Check { app: "pro-editor", issuers: &signer, roots: &f.store.roots, device: &dev, account: "alice", now: NOW, revocations: &revs };
        assert!(matches!(stored.verify(&check), Err(LicenseError::Revoked(_))));
    }
}
