//! A call or an answer bigger than a line (64 KiB) over the real loopback transport (F-SPLIT-03, D-SPLIT-03-STORE): the caller notices nothing,
//! the bytes go by the upload and download objects, and the objects are let go of afterwards.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_core_api::contract::RateLimit;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served as Server};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{serve_uploads, Host, Obj, Registry, Rpc, Served};
use serde_json::{json, Value};

/// An object that answers the size of the text it was sent and, for "make", a text of the size it was asked for.
struct Echo;
impl Served for Echo {
    fn iface(&self) -> &'static str {
        "Echo"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "size" => Ok(json!(a[0].as_str().map_or(0, str::len))),
            "make" => Ok(json!("x".repeat(a[0].as_u64().unwrap_or(0) as usize))),
            "both" => Ok(json!(format!("{}{}", a[0].as_str().unwrap_or(""), a[0].as_str().unwrap_or("")))),
            other => Err(format!("Echo has no call {other}")),
        }
    }
}

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    host: Arc<Host>,
}
impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn machine() -> Machine {
    let host = Arc::new(Host::new().with_root(serve_uploads()).with_root(Obj::Served(Arc::new(Echo))));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Server { hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }), settings: None, vault: None, flows: None, assistant: None, cameras: None, rpc: Some(host.clone()), appevents: None, ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]) };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, thread, host }
}

fn rpc(m: &Machine) -> Rpc {
    Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap())
}

#[test]
fn arguments_and_answers_bigger_than_a_line_cross_whole_and_a_small_call_stays_inline() {
    let m = machine();
    let rpc = rpc(&m);
    // small both ways: no object is made
    assert_eq!(rpc.call::<usize>("Echo", 0, "size", json!(["abc"])).unwrap(), 3);
    assert_eq!(rpc.call::<String>("Echo", 0, "make", json!([10])).unwrap(), "xxxxxxxxxx");
    // big arguments (over 64 KiB, several chunks) arrive whole
    let big = "ab".repeat(120_000);
    assert_eq!(rpc.call::<usize>("Echo", 0, "size", json!([big])).unwrap(), 240_000);
    // a big answer is read whole
    let got = rpc.call::<String>("Echo", 0, "make", json!([300_000])).unwrap();
    assert_eq!((got.len(), got.bytes().all(|b| b == b'x')), (300_000, true));
    // big both ways in one call
    let both = rpc.call::<String>("Echo", 0, "both", json!(["q".repeat(100_000)])).unwrap();
    assert_eq!(both.len(), 200_000);
    // the objects of the transfers are let go of: the connection holds nothing
    assert_eq!(m.host.held_total(), 0, "the upload and download objects are released");
}

#[test]
fn a_refusal_of_the_core_arrives_as_the_cores_words_even_after_a_big_upload() {
    let m = machine();
    let rpc = rpc(&m);
    let e = rpc.call::<usize>("Echo", 0, "nope", json!(["z".repeat(100_000)])).unwrap_err();
    assert!(e.contains("Echo has no call nope"), "{e}");
    assert_eq!(m.host.held_total(), 0);
}
