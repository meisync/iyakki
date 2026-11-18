//! Runs the page's real speech code in a real headless Chrome (through the DevTools protocol, driven by node): the voice list,
//! then a piece spoken after a genuine click, and the start and end events coming back. Ignored by default because it needs
//! `google-chrome` and `node` (22 or newer). It does not check that anything is audible: a headless browser has no speaker.
//! Run: cargo test -p iyakki-remote --test chrome_speech -- --ignored --nocapture

const DRIVER: &str = r##"const [DIR, PORT] = process.argv.slice(2);
import { spawn } from "node:child_process";
const chrome = spawn("google-chrome", ["--headless=new","--no-sandbox","--disable-gpu","--remote-debugging-port="+PORT,"--user-data-dir="+DIR+"/prof","about:blank"], {stdio:"ignore"});
const sleep = (ms)=>new Promise(r=>setTimeout(r,ms));
let tabs; for (let i=0;i<50;i++){ try { tabs = await (await fetch("http://127.0.0.1:"+PORT+"/json")).json(); if (tabs.length) break; } catch(e){} await sleep(200); }
const ws = new WebSocket(tabs.find(t=>t.type==="page").webSocketDebuggerUrl);
await new Promise(r=>ws.onopen=r);
let id=0; const pending=new Map();
ws.onmessage=(m)=>{const j=JSON.parse(m.data); if(j.id&&pending.has(j.id)){pending.get(j.id)(j);}};
const call=(method,params={})=>new Promise(r=>{const i=++id; pending.set(i,r); ws.send(JSON.stringify({id:i,method,params}));});
await call("Page.enable");
await call("Page.navigate",{url:"file://"+DIR+"/t.html"});
await sleep(1500);
// a real click gives the page user activation
await call("Input.dispatchMouseEvent",{type:"mousePressed",x:10,y:10,button:"left",clickCount:1});
await call("Input.dispatchMouseEvent",{type:"mouseReleased",x:10,y:10,button:"left",clickCount:1});
await call("Runtime.evaluate",{expression:"go()",userGesture:true});
await sleep(9000);
const r = await call("Runtime.evaluate",{expression:"out.filter(x=>!x.includes('sp_voices')).join('\\n')",returnByValue:true});
console.log(r.result.result.value);
chrome.kill();
process.exit(0);
"##;

#[test]
#[ignore]
fn the_page_speaks_a_piece_in_a_real_chrome_and_reports_start_and_end() {
    let page = iyakki_remote::page::INDEX;
    let from = page.find("// Browser voice:").expect("speech code");
    let to = page.find("function size()").expect("end of speech code");
    let html = format!(
        "<!doctype html><meta charset=utf-8><body><script>const out=[]; function send(o){{ out.push(JSON.stringify(o)); }}\n{}\nasync function go(){{ spVoices(); await new Promise(r=>setTimeout(r,1500)); spVoices(); spPrime(); spMessage({{t:'speak',id:1,text:'Hello from the story.',lang:'en-US',rate:1.5}}); }}</script>",
        &page[from..to]
    );
    let dir = std::env::temp_dir().join(format!("iyakki-remote-speech-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("t.html"), html).unwrap();
    std::fs::write(dir.join("cdp.mjs"), DRIVER).unwrap();
    let port = 9400 + std::process::id() % 500;
    let out = std::process::Command::new("node").arg(dir.join("cdp.mjs")).arg(dir.to_string_lossy().as_ref()).arg(port.to_string()).output().expect("node");
    let _ = std::fs::remove_dir_all(&dir);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    eprintln!("{text}");
    assert!(text.contains(r#""t":"sp_start","id":1"#) && text.contains(r#""t":"sp_end","id":1"#), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
}
