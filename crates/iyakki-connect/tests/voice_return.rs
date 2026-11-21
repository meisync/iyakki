// SPDX-License-Identifier: Apache-2.0
// Copyright IYAKKI contributors. Licensed under the Apache License, Version 2.0 (see LICENSE at the root of the repository).

//! The connector posts the device's return offer (F-VOICE-61) to a fake tenant over real TLS. Its own test binary: the fake tenant's voice
//! statics belong to one test.

mod common;

use std::io::Cursor;
use std::time::Duration;

use iyakki_connect::{pins::Store, run, Ctx, Fail};

fn go(c: &Ctx, input: &str, args: &[&str]) -> (Result<(), Fail>, String) {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let r = run(&a, c, &mut Cursor::new(input.to_string()), &mut out);
    (r, String::from_utf8(out).unwrap())
}

#[test]
fn the_return_offer_is_posted_in_the_signed_in_session_and_a_server_without_the_route_is_told_in_words() {
    let r = common::start_remote();
    let d = std::env::temp_dir().join(format!("iyakki-connect-test-voice-return-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let c = Ctx { store: Store::new(&d), timeout: Duration::from_secs(5), interactive: false };
    let addr = &r.target.addr;
    go(&c, "", &["pin", addr, "--yes"]).0.unwrap();
    let target = iyakki_link::cloudclient::Target { addr: addr.clone(), pin: c.store.get(addr).unwrap().unwrap() };
    let api = iyakki_link::rawlink::FilesApi::login(&target, common::USER, common::PASSWORD).unwrap();
    let e = api.voice_return("198.51.100.4:7444", "{}").unwrap_err();
    assert!(format!("{e:?}").contains("does not take a microphone"), "{e:?}");
    *common::VOICE_OFFER.lock().unwrap() = "{\"addr\":\"x:1\",\"offer\":\"y\"}".into();
    api.voice_return("198.51.100.4:7444", "{\"machine\":\"laptop\"}").unwrap();
    let got = common::VOICE_RETURNS.lock().unwrap().clone();
    let v: serde_json::Value = serde_json::from_str(&got[0]).unwrap();
    assert_eq!((v["addr"].as_str(), v["offer"].as_str()), (Some("198.51.100.4:7444"), Some("{\"machine\":\"laptop\"}")));
    // an address that is empty, too long or holds a control character never leaves this computer
    for bad in ["", "a\nb", &"9".repeat(300)] {
        assert!(api.voice_return(bad, "{}").is_err());
    }
    assert_eq!(common::VOICE_RETURNS.lock().unwrap().len(), 1);
}
