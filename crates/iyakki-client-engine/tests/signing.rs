//! The engine's answer to the instance's challenge must be exactly what `clientlink::sign_answer` makes and `verify_answer` accepts.
//! `iyakki_agent::clientlink::sign_answer` is one line over `iyakki_link::sigscheme::DeviceKey::sign_request` (method GET, path `/client/ws`,
//! body the challenge), so the reference here is that function with a fixed nonce. The golden vectors in `golden/sig.json` are shared with
//! the node test of the glue (`js/test/glue.test.mjs`), which signs through the real wasm module and compares to the same file.

use iyakki_client_engine::sign;
use iyakki_core_api::client::WS_PATH;
use iyakki_link::sigscheme::{parse_authorization, DeviceKey};
use serde_json::Value;

fn vectors() -> Vec<Value> {
    serde_json::from_str::<Value>(include_str!("golden/sig.json")).unwrap()["vectors"].as_array().unwrap().clone()
}

fn seed_of(v: &Value) -> [u8; 32] {
    <[u8; 32]>::try_from(sign::unhex(v["seed"].as_str().unwrap()).unwrap()).unwrap()
}

#[test]
fn the_engine_signs_exactly_what_the_instances_scheme_signs() {
    for v in vectors() {
        let (device, challenge, nonce, ts) = (v["device"].as_str().unwrap(), v["challenge"].as_str().unwrap(), v["nonce"].as_str().unwrap(), v["ts"].as_u64().unwrap());
        let seed = seed_of(&v);
        let ours = sign::sign_with_seed(&seed, device, challenge, ts, nonce);
        let reference = DeviceKey::from_seed(seed).sign_request_with_nonce(device, "GET", WS_PATH, ts, challenge.as_bytes(), nonce).unwrap();
        assert_eq!(ours, reference, "{v}");
        assert_eq!(sign::hex(&sign::public_from_seed(&seed)), DeviceKey::from_seed(seed).public_hex());
        assert_eq!(ours, v["authorization"].as_str().unwrap(), "the golden vector moved: {v}");
        assert_eq!(sign::request_text(device, challenge, ts, nonce), v["text"].as_str().unwrap());
    }
}

#[test]
fn what_the_instance_checks_accepts_the_engines_answer_and_nothing_else() {
    for v in vectors() {
        let (device, challenge, nonce, ts) = (v["device"].as_str().unwrap(), v["challenge"].as_str().unwrap(), v["nonce"].as_str().unwrap(), v["ts"].as_u64().unwrap());
        let seed = seed_of(&v);
        let public = sign::public_from_seed(&seed);
        let parsed = parse_authorization(&sign::sign_with_seed(&seed, device, challenge, ts, nonce)).expect("the header parses");
        assert_eq!((parsed.id.as_str(), parsed.ts), (device, ts));
        assert!(parsed.verify(&public, "GET", WS_PATH, challenge.as_bytes()));
        assert!(!parsed.verify(&public, "GET", WS_PATH, b"another challenge"), "bound to the challenge");
        assert!(!parsed.verify(&public, "GET", "/other", challenge.as_bytes()), "bound to the path");
        assert!(!parsed.verify(&[9u8; 32], "GET", WS_PATH, challenge.as_bytes()), "bound to the key");
    }
}

#[test]
fn a_signature_made_elsewhere_is_put_into_the_same_header() {
    // the WebCrypto path: the engine builds the text and the header, the browser signs
    let seed = [5u8; 32];
    let text = sign::request_text("dabc", "ch", 1_700_000_000, "00112233445566778899aabbccddeeff");
    use ed25519_dalek::Signer;
    let sig = ed25519_dalek::SigningKey::from_bytes(&seed).sign(text.as_bytes()).to_bytes();
    let h = sign::header("dabc", 1_700_000_000, "00112233445566778899aabbccddeeff", &sig);
    assert_eq!(h, sign::sign_with_seed(&seed, "dabc", "ch", 1_700_000_000, "00112233445566778899aabbccddeeff"));
}
