//! The page in a real headless Chrome (driven through the DevTools protocol by node) against `serve` and a FAKE tenant, and the
//! loopback figures. Ignored by default: it needs `google-chrome` and `node` (22 or newer).
//!   cargo test --release -p iyakki-connect --test chrome -- --ignored --nocapture
//! What it proves: the page loads under its own Content-Security-Policy (no violation), the worker draws the tenant's tiles onto the
//! canvas (pixels read back from a screenshot), mouse and key events reach the tenant in its own words. What it does not: a real
//! desktop, sound/camera/clipboard (they need user gestures and devices), a real network.

mod common;

use common::*;
use iyakki_connect::serve::{Options, Server};
use iyakki_link::rawlink::Auth;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const DRIVER: &str = r##"const [DIR, PORT, URL, FRAMES] = process.argv.slice(2);
import { spawn } from "node:child_process";
import zlib from "node:zlib";
const chrome = spawn("google-chrome", ["--headless=new","--no-sandbox","--disable-gpu","--window-size=800,600","--force-device-scale-factor=1","--remote-debugging-port="+PORT,"--user-data-dir="+DIR+"/prof","about:blank"], {stdio:"ignore"});
const sleep = (ms)=>new Promise(r=>setTimeout(r,ms));
let tabs; for (let i=0;i<60;i++){ try { tabs = await (await fetch("http://127.0.0.1:"+PORT+"/json")).json(); if (tabs.length) break; } catch(e){} await sleep(200); }
const ws = new WebSocket(tabs.find(t=>t.type==="page").webSocketDebuggerUrl);
await new Promise(r=>ws.onopen=r);
let id=0; const pending=new Map(); const events=[];
ws.onmessage=(m)=>{const j=JSON.parse(m.data); if(j.id&&pending.has(j.id)){pending.get(j.id)(j);} else events.push(j);};
const call=(method,params={})=>new Promise(r=>{const i=++id; pending.set(i,r); ws.send(JSON.stringify({id:i,method,params}));});
const ev=async(e)=>{const r=await call("Runtime.evaluate",{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:undefined;};
await call("Page.enable"); await call("Runtime.enable"); await call("Log.enable");
const t0=Date.now();
await call("Page.navigate",{url:URL});
let frames=0; for(let i=0;i<300;i++){ frames=await ev("window.__frames||0"); if(frames>=1) break; await sleep(50); }
const firstFrameMs=Date.now()-t0;
// the picture: read back through a screenshot, since the canvas lives in a worker
async function pixels(){ const r=await call("Page.captureScreenshot",{format:"png"}); return decodePng(Buffer.from(r.result.data,"base64")); }
function decodePng(b){ let p=8,w=0,h=0,ct=0,idat=[]; while(p<b.length){const len=b.readUInt32BE(p),ty=b.toString("ascii",p+4,p+8); if(ty==="IHDR"){w=b.readUInt32BE(p+8);h=b.readUInt32BE(p+12);ct=b[p+17];} if(ty==="IDAT")idat.push(b.subarray(p+8,p+8+len)); p+=12+len;}
  const bpp=ct===6?4:3, raw=zlib.inflateSync(Buffer.concat(idat)), row=w*bpp, out=Buffer.alloc(h*row);
  for(let y=0;y<h;y++){const f=raw[y*(row+1)];for(let i=0;i<row;i++){const x=raw[y*(row+1)+1+i],a=i>=bpp?out[y*row+i-bpp]:0,u=y?out[(y-1)*row+i]:0,c=(y&&i>=bpp)?out[(y-1)*row+i-bpp]:0;let v;
    if(f===0)v=x;else if(f===1)v=x+a;else if(f===2)v=x+u;else if(f===3)v=x+((a+u)>>1);else{const pp=a+u-c,pa=Math.abs(pp-a),pb=Math.abs(pp-u),pc=Math.abs(pp-c);v=x+(pa<=pb&&pa<=pc?a:pb<=pc?u:c);} out[y*row+i]=v&255;}}
  return {w,h,at:(x,y)=>[out[y*row+x*bpp],out[y*row+x*bpp+1],out[y*row+x*bpp+2]]}; }
await sleep(500);
const img=await pixels();
const sample={ gradient: img.at(10,10), gradient2: img.at(300,200), box: img.at(10+5,100+5) };
// input: a real mouse move and a real key press on the canvas
await call("Input.dispatchMouseEvent",{type:"mouseMoved",x:123,y:77});
await call("Input.dispatchMouseEvent",{type:"mousePressed",x:123,y:77,button:"left",clickCount:1});
await call("Input.dispatchMouseEvent",{type:"mouseReleased",x:123,y:77,button:"left",clickCount:1});
await call("Input.dispatchKeyEvent",{type:"keyDown",key:"a",code:"KeyA",text:"a"});
await call("Input.dispatchKeyEvent",{type:"keyUp",key:"a",code:"KeyA"});
// the frame loop: the tenant sends one frame per acknowledgement
for(let i=0;i<600;i++){ frames=await ev("window.__frames||0"); if(frames>=+FRAMES) break; await sleep(50); }
const ft=await ev("JSON.stringify(window.__ft||[])");
const nav=await ev("JSON.stringify(performance.getEntriesByType('navigation')[0])");
const csp=events.filter(e=>(e.method==="Log.entryAdded"&&/Content Security Policy|Refused to/i.test(e.params.entry.text))||(e.method==="Runtime.exceptionThrown")).map(e=>JSON.stringify(e.params).slice(0,300));
const errs=events.filter(e=>e.method==="Log.entryAdded"&&e.params.entry.level==="error").map(e=>e.params.entry.text.slice(0,200));
const ua=await ev("navigator.userAgent");
console.log(JSON.stringify({firstFrameMs,frames,sample,ft:JSON.parse(ft||"[]"),nav:JSON.parse(nav||"{}"),csp,errs,ua,w:img.w,h:img.h}));
chrome.kill(); process.exit(0);
"##;

#[test]
#[ignore]
fn the_page_loads_draws_the_tenants_tiles_and_sends_input_in_a_real_chrome() {
    let r = start_remote();
    r.frames_on_ack.store(200, Ordering::SeqCst);
    let s = Server::start(Options { target: r.target.clone(), auth: Auth::Password { user: USER.into(), password: zeroize::Zeroizing::new(PASSWORD.into()) }, share: None, files: None, port: 0 }).unwrap();
    let dir = std::env::temp_dir().join(format!("iyakki-connect-chrome-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("cdp.mjs"), DRIVER).unwrap();
    let port = 9500 + std::process::id() % 400;
    let out = std::process::Command::new("node").arg(dir.join("cdp.mjs")).args([dir.to_string_lossy().as_ref(), &port.to_string(), &s.url(), "200"]).output().expect("node");
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!text.is_empty(), "no result: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap_or_else(|e| panic!("{e}: {text}"));
    eprintln!("browser: {}", v["ua"]);
    // pixels: the gradient of the fake tenant's picture, and the yellow box of the first frame (or a later one that has moved on)
    let near = |p: &serde_json::Value, want: [i64; 3], tol: i64| (0..3).all(|i| (p[i].as_i64().unwrap() - want[i]).abs() <= tol);
    eprintln!("sample pixels: {}", v["sample"]);
    assert!(near(&v["sample"]["gradient"], [3, 6, 90], 3), "the picture was drawn: {}", v["sample"]);
    assert!(near(&v["sample"]["gradient2"], [(300 * 255 / 640) as i64, (200 * 255 / 400) as i64, 90], 3), "{}", v["sample"]);
    // nothing in the page broke its own policy
    assert_eq!(v["csp"].as_array().unwrap().len(), 0, "policy violations or exceptions: {}", v["csp"]);
    // input reached the tenant in its own words
    wait_for("input at the tenant", 5, || r.texts.lock().unwrap().iter().any(|t| t.contains(r#""t":"key""#)));
    let texts = r.texts.lock().unwrap().clone();
    assert!(texts.iter().any(|t| t.contains(r#""t":"down""#)), "a click: {texts:?}");
    assert!(texts.iter().any(|t| t.contains(r#""key":"a""#) && t.contains(r#""code":"KeyA""#)), "a key: {texts:?}");
    assert!(texts.iter().any(|t| t.contains(r#""t":"hello""#)) && texts.iter().any(|t| t.contains(r#""t":"ack""#)));
    // figures
    let ft: Vec<f64> = v["ft"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    let mut gaps: Vec<f64> = ft.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| gaps.get(((gaps.len() as f64 - 1.0) * p) as usize).copied().unwrap_or(f64::NAN);
    eprintln!(
        "FIGURES (fake tenant, loopback, headless Chrome): first frame drawn {} ms after navigation start; DOMContentLoaded {:.0} ms, load {:.0} ms; {} frames: frame-to-frame (ack out, next frame in and drawn) median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        v["firstFrameMs"], v["nav"]["domContentLoadedEventEnd"].as_f64().unwrap_or(f64::NAN), v["nav"]["loadEventEnd"].as_f64().unwrap_or(f64::NAN), ft.len(), pct(0.5), pct(0.95), pct(1.0)
    );
    assert!(ft.len() >= 100, "the frame loop ran: {} frames", ft.len());
}

/// The relay alone, without a browser: how long from an `ack` written to the page server's socket to the next frame arriving back on
/// it, through the program and a fake tenant over TLS. A FAKE tenant (frames are built before the run); no decode, no drawing.
#[test]
#[ignore]
fn relay_round_trip_without_a_browser() {
    let r = start_remote();
    r.frames_on_ack.store(300, Ordering::SeqCst);
    let s = Server::start(Options { target: r.target.clone(), auth: Auth::Password { user: USER.into(), password: zeroize::Zeroizing::new(PASSWORD.into()) }, share: None, files: None, port: 0 }).unwrap();
    let token = s.url().split("?t=").nth(1).unwrap().to_string();
    let (_, head, _) = get(s.port(), &format!("/?t={token}"), None, "");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let mut w = Ws::open(s.port(), &cookie, Some(&format!("http://127.0.0.1:{}", s.port()))).unwrap();
    w.send_text(r#"{"t":"hello","v":2,"caps":{"deflate":true}}"#);
    let mut got = 0usize;
    let mut bytes = 0usize;
    let mut times = Vec::new();
    let mut t = Instant::now();
    while got < 301 {
        match w.next(5) {
            Got::Binary(b) => {
                if got > 0 {
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                }
                got += 1;
                bytes += b.len();
                w.send_text(&format!(r#"{{"t":"ack","n":{got}}}"#));
                t = Instant::now();
            }
            Got::Eof => panic!("the stream ended after {got}"),
            _ => {}
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| times[((times.len() as f64 - 1.0) * p) as usize];
    eprintln!(
        "RELAY (fake tenant, TLS, loopback, no decode): {} frames, {:.1} kB average; ack-to-next-frame median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        got,
        bytes as f64 / got as f64 / 1000.0,
        pct(0.5),
        pct(0.95),
        pct(1.0)
    );
    let _ = Duration::from_millis(1);
}
