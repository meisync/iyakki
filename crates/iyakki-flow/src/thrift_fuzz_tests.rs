//! Fuzz tests (deterministic, seeded) for the Thrift JSON codec: mutated replies through `decode_reply`, and random type descriptions and
//! values through `parse_args` and `encode_call` (what is written must be valid JSON whatever the values were).
//! `IYAKKI_FUZZ_CASES=50000 cargo test -p iyakki-flow fuzz` runs a larger set.

use super::*;
use crate::fuzzkit::{cases, run, Rng};
use std::time::Duration;

fn replies() -> Vec<Vec<u8>> {
    [
        r#"[1,"add",2,1,{"0":{"i32":5}}]"#,
        r#"[1,"Calc:get",2,7,{"0":{"rec":{"1":{"str":"Ada"},"2":{"lst":["i32",3,1,2,3]},"3":{"map":["str","dbl",1,{"a":0.5}]},"4":{"set":["str",1,"x"]}}}}]"#,
        r#"[1,"m",2,1,{"1":{"rec":{"1":{"str":"declared"}}}}]"#,
        r#"[1,"m",3,1,{"1":{"str":"boom"},"2":{"i32":6}}]"#,
        r#"[1,"m",2,1,{"0":{"dbl":"NaN"},"1":{"tf":1},"2":{"i8":-5}}]"#,
        &format!("[1,\"m\",2,1,{{\"0\":{}0{}}}]", "{\"lst\":[\"lst\",1,".repeat(40), "]}".repeat(40)),
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect()
}

#[test]
fn fuzz_thrift_replies_never_panic_or_balloon() {
    let corpus = replies();
    let n = run("thrift reply", 0x7F1, &corpus, 20000, Duration::from_secs(1), 64 << 20, |input| {
        let text = String::from_utf8_lossy(input);
        for (m, s) in [("add", 1), ("m", 1), ("get", 7)] {
            if let Ok(Reply::Application { kind, .. }) = decode_reply(&text, m, s) {
                let _ = exception_name(kind);
            }
        }
    });
    assert!(n >= cases(20000));
}

fn rand_ty(r: &mut Rng, depth: usize) -> Value {
    let simple = ["bool", "byte", "i8", "i16", "i32", "i64", "double", "string", "binary", "float", "BOOL"];
    if depth > 30 || r.chance(2) {
        return json!(*r.pick(&simple));
    }
    match r.below(5) {
        0 => json!({"list": rand_ty(r, depth + 1)}),
        1 => json!({"set": rand_ty(r, depth + 1)}),
        2 => json!({"map": [rand_ty(r, depth + 1), rand_ty(r, depth + 1)]}),
        3 => json!({"struct": (0..r.below(4)).map(|i| json!({"id": if r.chance(8) { 0 } else { i + 1 }, "name": "f", "type": rand_ty(r, depth + 1)})).collect::<Vec<_>>()}),
        _ => json!({"unknown": 1, "other": 2}),
    }
}

fn rand_val(r: &mut Rng, depth: usize) -> Value {
    if depth > 30 || r.chance(2) {
        return match r.below(12) {
            0 => json!(null),
            1 => json!(true),
            2 => json!(r.next() as i64),
            3 => json!(r.below(300) as i64 - 100),
            4 => json!(u64::MAX),
            5 => json!(1.5e300),
            6 => json!("NaN"),
            7 => json!("text \" \\ \n \u{0} </script> \u{2028}"),
            8 => json!("AAEC"),
            9 => json!("9007199254740993"),
            10 => json!(""),
            _ => json!(-0.0),
        };
    }
    match r.below(3) {
        0 => Value::Array((0..r.below(6)).map(|_| rand_val(r, depth + 1)).collect()),
        1 => Value::Object((0..r.below(5)).map(|i| (if r.chance(2) { format!("{}", i + 1) } else { format!("k\"{i}") }, rand_val(r, depth + 1))).collect()),
        _ => json!({"x": rand_val(r, depth + 1)}),
    }
}

#[test]
fn fuzz_thrift_encode_writes_valid_json_for_any_arguments() {
    let mut r = Rng::new(0x7F2);
    let mut written = 0usize;
    for case in 0..cases(8000) {
        let list: Vec<Value> = (0..r.below(5)).map(|i| json!({"id": i + 1, "type": rand_ty(&mut r, 0), "value": rand_val(&mut r, 0)})).collect();
        let Ok(args) = parse_args(&Value::Array(list.clone())) else { continue };
        let svc = if r.chance(4) { "Svc.Name" } else { "" };
        let t = std::time::Instant::now();
        if let Ok(text) = encode_call(svc, "method_1", r.below(1000) as i32 - 5, r.chance(4), &args) {
            written += 1;
            let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("case {case}: the call is not valid JSON ({e}): {text}\nfrom {}", Value::Array(list.clone())));
            assert!(v.as_array().is_some_and(|a| a.len() == 5 && a[0] == json!(1)), "case {case}: {text}");
        }
        assert!(t.elapsed() < Duration::from_secs(2), "case {case} took {:?}", t.elapsed());
    }
    assert!(written > 200, "enough random calls must be valid ({written})");
}

#[test]
fn fuzz_thrift_deep_and_wide_inputs_are_refused_not_followed() {
    let mut ty = json!("i32");
    for _ in 0..200 {
        ty = json!({"list": ty});
    }
    assert!(parse_ty(&ty, 0).is_err());
    let mut v = json!(1);
    for _ in 0..200 {
        v = json!([v]);
    }
    let a = vec![(Field { id: 1, name: String::new(), ty: Ty::List(Box::new(Ty::List(Box::new(Ty::I32)))) }, v)];
    let _ = encode_call("", "m", 1, false, &a);
    let wide = Value::Array((0..300).map(|i| json!({"id": i + 1, "type": "i32"})).collect());
    assert!(parse_args(&wide).is_err());
}
