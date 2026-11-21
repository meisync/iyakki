//! The whole shell over a FAKE core's socket (F-SPLIT-03 layer 1): the reference transport server (`iyakki_ui_client::server::serve_unix`) with no
//! contract domains of its own and one host of remote objects over a throwaway device. The program side is the same as against the real core; what
//! is fake is the core's contract: no settings, vault or flow adapters, so the program's `Core` handle answers quietly for those and the shell still
//! draws, signs in and changes a setting. Linux only.
#![cfg(all(target_os = "linux", feature = "socket-shell"))]

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::{drive, Device};
use iyakki_core_api::contract::RateLimit;
use iyakki_ui_bin::socket_shell::environment;
use iyakki_ui_client::server::{serve_unix, settings_hello, RpcServe, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};

#[test]
fn the_shell_draws_signs_in_and_changes_a_setting_against_a_fake_core_over_a_unix_socket() {
    let dev = Device::new("fake");
    let path = dev.dir.join("fake.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let host: Arc<dyn RpcServe> = dev.host();
    let served = Served {
        hello: settings_hello("fake-core", RateLimit { calls: 1_000_000, per_ms: 1000 }),
        settings: None,
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        appevents: None,
        rpc: Some(host),
        ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::spawn(move || serve_unix(listener, Arc::new(served), s2).unwrap());
    let env = environment(&path, false, 1, &|| {}).unwrap();
    assert_eq!(env.hostname, "fake-core", "the machine's name is the core's");
    let mut sh = iyakki_shell::shell::Shell::new(env);
    drive(&mut sh, &dev);
    let (opened, panicked, unserved) = common::sweep_unserved(&mut sh);
    println!("opened {} apps: {opened:?}", opened.len());
    assert!(panicked.is_empty(), "apps that panicked over the socket: {panicked:#?}");
    assert!(opened.len() >= 20, "most apps open: {opened:?}");
    for id in ["radio", "media", "lab", "rag", "inbox", "system", "cli"] {
        assert!(opened.contains(&id) && !unserved.contains(&id), "{id} opens over the reference server and is not the unserved screen: unserved {unserved:?}");
    }
    common::tile_apps_round_trip(&mut sh, &dev);
    drop(sh);
    stop.store(true, Ordering::Relaxed);
    thread.join().unwrap();
}
