//! The page JavaScript engine (QuickJS-ng in our WebAssembly sandbox) against a real, independent JavaScript engine (node, V8):
//! the same snippets must give the same text. Language and library behaviour only (no page, no browser). Skipped, with a message,
//! when `node` is not installed.

use iyakki_js::{JsEngine, Limits};
use std::process::Command;

fn node(src: &str) -> Option<String> {
    let o = Command::new("node").args(["-e", "process.stdout.write(String(eval(process.argv[1])))", src]).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

const SNIPPETS: &[&str] = &[
    "1 + 2 * 3 - 4 / 8",
    "[3,1,2].sort().join('')",
    "[10,9,1].sort().join(',')",
    "[10,9,1].sort((a,b)=>a-b).join(',')",
    "JSON.stringify({a:[1,{b:null}],c:'x\\u2028y',d:undefined,e:1e21})",
    "JSON.stringify(JSON.parse('{\"k\":[1,2,{\"z\":true}],\"s\":\"\\\\u00e9\"}'))",
    "'Straße'.toUpperCase() + 'İ'.toLowerCase().length",
    "'a-b_c d'.split(/[-_ ]/).map(s=>s.padStart(3,'*')).join('|')",
    "'2026-11-16'.replace(/(\\d+)-(\\d+)-(\\d+)/, '$3.$2.$1')",
    "/(?<y>\\d{4})-(?<m>\\d\\d)/.exec('on 2026-11-16').groups.m",
    "[...'a😀b'].length + ':' + 'a😀b'.length",
    "new Date(Date.UTC(2026, 10, 16, 12, 30, 5)).toISOString()",
    "new Date('2026-02-29T00:00:00Z').getTime()",
    "(1234.5678).toFixed(2) + ' ' + (0.1+0.2) + ' ' + (255).toString(16) + ' ' + (1e21).toString()",
    "parseInt('08') + parseFloat('3.14abc') + Number('0x10') + Number('') + Number(' 12 ')",
    "String(2**53 + 2) + ' ' + String(-0) + ' ' + Object.is(-0, 0) + ' ' + (0/0)",
    "12345678901234567890n * 98765432109876543210n",
    "typeof null + typeof undefined + typeof (()=>1) + typeof Symbol() + typeof 1n",
    "[1,[2,[3,[4]]]].flat(Infinity).join()",
    "Object.entries({b:2,a:1,1:'x'}).map(e=>e.join('=')).join('&')",
    "new Map([[1,'a'],[2,'b']]).get(2) + [...new Set([3,3,4])].join('')",
    "Array.from({length:5},(_, i)=>i*i).reduce((a,b)=>a+b)",
    "class A { static #n = 0; static next() { return ++A.#n; } } A.next() + A.next()",
    "(function(){ try { null.x } catch (e) { return e instanceof TypeError } })()",
    "(function(){ let o = {}; o.self = o; try { JSON.stringify(o) } catch (e) { return e.name } })()",
    "((a, {b = 2, ...r} = {}, ...z) => [a, b, Object.keys(r).join(), z.length])(1, {c:3, d:4}, 5, 6).join(';')",
    "[...'abc'].map((c,i)=>c.charCodeAt(0)+i).join()",
    "encodeURIComponent('a b&c=é/') + decodeURIComponent('%E2%82%AC')",
    "escape('ä b') + unescape('%u20AC')",
    "'abc'.at(-1) + [1,2,3].at(-1) + Object.fromEntries([['a',1]]).a + 'x'.repeat(3)",
    "[1,2,3,4].findLast(x=>x%2) + [5,6].toSorted?.((a,b)=>b-a).join('')",
    "String(new Error('boom', {cause: 'why'}).cause)",
    "label: { let s = 0; for (let i = 0; i < 10; i++) { if (i == 7) break label; s += i } s }",
    "(function*(){ yield 1; yield* [2,3]; return 4 })().next().value + [...(function*(){ yield 1; yield* [2,3]; })()].join('')",
    "Number.MAX_SAFE_INTEGER + ',' + Number.EPSILON.toExponential(3) + ',' + Math.hypot(3,4) + ',' + Math.sign(-3) + ',' + Math.trunc(-4.7)",
    "'x'.localeCompare('y') + ',' + 'a'.codePointAt(0) + ',' + String.fromCodePoint(128512).length",
    "Reflect.ownKeys({b:1,[Symbol.for('s')]:2,a:3, 2:4}).map(String).join()",
    "(new Proxy({}, { get: (t, k) => 'p:' + String(k) })).hello",
    "new Uint8Array([1,2,300]).join() + new Float32Array([0.1])[0] + new DataView(new ArrayBuffer(4)).byteLength",
    "atob(btoa('héllo'.replace('é','e'))) + btoa('abc')",
];

#[test]
fn the_same_snippets_give_the_same_text_as_node() {
    if node("1").is_none() {
        eprintln!("SKIPPED: node is not installed");
        return;
    }
    let mut e = JsEngine::new(Limits::default(), vec![]).unwrap();
    let mut different = Vec::new();
    let mut compared = 0;
    for s in SNIPPETS {
        let Some(want) = node(s) else {
            // node itself refuses it (an old node without a feature): not a comparison
            continue;
        };
        compared += 1;
        let got = e.eval("diff", &format!("String(eval({}))", serde_json::to_string(s).unwrap()));
        match got {
            Ok(g) if g == want => {}
            Ok(g) => different.push(format!("{s}\n   node: {want}\n   ours: {g}")),
            Err(err) => different.push(format!("{s}\n   node: {want}\n   ours threw: {err:?}")),
        }
    }
    assert!(compared >= SNIPPETS.len() - 4, "node ran only {compared} of {} snippets", SNIPPETS.len());
    assert!(different.is_empty(), "{} of {compared} snippets differ from node:\n{}", different.len(), different.join("\n"));
}

#[test]
fn promises_and_microtasks_run_in_the_same_order_as_in_node() {
    if node("1").is_none() {
        eprintln!("SKIPPED: node is not installed");
        return;
    }
    let script = "var log = []; Promise.resolve().then(()=>log.push('p1')).then(()=>log.push('p2')); queueMicrotask(()=>log.push('m')); (async()=>{ log.push('a0'); await null; log.push('a1'); })(); log.push('sync');";
    let finish = "log.join(',')";
    // node: run the script, let the jobs finish, print the log
    let o = Command::new("node").args(["-e", &format!("{script} setImmediate(()=>process.stdout.write({finish}))")]).output().unwrap();
    let want = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(want.starts_with("a0,sync,"), "{want}");
    let mut e = JsEngine::new(Limits::default(), vec![]).unwrap();
    e.eval("order", script).unwrap();
    let _ = e.run_jobs();
    let ours = e.eval("order", finish).unwrap();
    assert_eq!(ours, want, "microtask order");
}
