#![cfg(target_os = "linux")]
//! The self test of the app network gate (the `selftest-netprobe` subcommand runs a probe inside a fresh namespace); moved out of `linuxapps_net` with its process starts.

use iyakki_shared::linuxapps_net::*;
use crate::qprintln;

    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    pub const SUBCOMMAND: &str = "selftest-netprobe";

    /// The probe process: new network namespace, say `ready`, wait for `go` on stdin, try each `label=address` and print `probe <label> ok|fail`.
    pub fn probe_main(args: &[String]) -> i32 {
        // SAFETY: unshare takes only the flag; this process has not started other threads.
        if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
            qprintln!("error unshare: {}", std::io::Error::last_os_error());
            return 2;
        }
        qprintln!("ready");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        for a in args {
            let Some((label, addr)) = a.split_once('=') else { continue };
            let ok = addr.parse::<std::net::SocketAddr>().is_ok_and(|s| std::net::TcpStream::connect_timeout(&s, Duration::from_secs(3)).is_ok());
            qprintln!("probe {label} {}", if ok { "ok" } else { "fail" });
        }
        let _ = std::io::stdout().flush();
        0
    }

    fn table_there(name: &str) -> bool {
        iyakki_fw::nl::NfSocket::open(5).and_then(|mut s| s.table_exists(name)).unwrap_or(false)
    }

    /// Run the whole check. `allow` and `deny` are TCP ports a listener on the guest's gateway address (10.0.2.2, QEMU's user network) answers.
    /// Returns `(name, passed, detail)`.
    pub fn run(allow: u16, deny: u16) -> Vec<(String, bool, String)> {
        let mut out: Vec<(String, bool, String)> = Vec::new();
        let mut add = |n: &str, ok: bool, d: String| out.push((n.to_string(), ok, d));
        let Ok(exe) = std::env::current_exe() else { return vec![("start".into(), false, "no executable path".into())] };
        let listener = std::net::TcpListener::bind("0.0.0.0:0");
        let Ok(listener) = listener else { return vec![("host-listener".into(), false, "could not listen".into())] };
        let _ = listener.set_nonblocking(true);
        let lport = listener.local_addr().map(|a| a.port()).unwrap_or(0);

        let slot = pick_slot("selftest", &[]).unwrap_or(7);
        let veth = Veth::for_slot(slot);
        let policy = NetPolicy { mode: Mode::OutboundOnly, allow: NetPolicy::parse_allow(&[format!("10.0.2.2:{allow}")]).unwrap_or_default(), slot, ..Default::default() };
        let before_forward = crate::sharing::real::ip_forward_get().unwrap_or_default();
        let before_ifs = interface_names();
        let table = table_name(&veth);
        add("clean-start", !table_there(&table) && !before_ifs.iter().any(|n| n == &veth.host_if), format!("forward={before_forward}"));

        let Ok(spec_table) = table_spec(&policy, &veth, &local_addresses(), &[], &[]) else { return vec![("table-spec".into(), false, "could not build the table".into())] };
        let spec_path = format!("/run/iyakki-selftest-net-{slot}.json");
        if std::fs::write(&spec_path, serde_json::to_string(&spec_table).unwrap_or_default()).is_err() {
            return vec![("table-spec".into(), false, "could not write the rules".into())];
        }
        let mut spec = crate::linuxapps_init::Spec::new("/", "selftest", vec!["x".into()]);
        spec.veth = Some(slot);
        spec.net_spec = spec_path.clone();
        spec.net_resolv = "/run/iyakki-selftest-resolv".into();

        let targets = [format!("allowed=10.0.2.2:{allow}"), format!("denied=10.0.2.2:{deny}"), format!("host={}:{lport}", veth.host_ip)];
        let child = Command::new(exe).arg(SUBCOMMAND).args(&targets).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else { return vec![("probe-start".into(), false, "could not start the probe".into())] };
        let mut reader = BufReader::new(child.stdout.take().expect("piped"));
        let mut first = String::new();
        let _ = reader.read_line(&mut first);
        add("probe-namespace", first.trim() == "ready", first.trim().to_string());
        let pid = child.id() as i32;

        let up = crate::linuxapps_init::net_up(&spec, pid);
        add("net-up", up.is_ok(), up.as_ref().err().cloned().unwrap_or_else(|| "ok".into()));
        let forward_before = up.ok().flatten();
        add("table-loaded", table_there(&table), table.clone());
        add("veth-host-end", interface_names().iter().any(|n| n == &veth.host_if), veth.host_if.clone());
        add("forwarding-on", crate::sharing::real::ip_forward_get().as_deref() == Ok("1"), "ip_forward=1".into());

        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"go\n");
            let _ = stdin.flush();
        }
        let mut seen: Vec<(String, bool)> = Vec::new();
        for l in reader.lines().map_while(Result::ok) {
            if let Some(rest) = l.strip_prefix("probe ") {
                let mut it = rest.split_whitespace();
                if let (Some(label), Some(v)) = (it.next(), it.next()) {
                    seen.push((label.to_string(), v == "ok"));
                }
            }
        }
        let got = |label: &str| seen.iter().find(|s| s.0 == label).map(|s| s.1);
        add("allowed-reachable", got("allowed") == Some(true), format!("10.0.2.2:{allow} -> {:?}", got("allowed")));
        add("not-listed-refused", got("denied") == Some(false), format!("10.0.2.2:{deny} -> {:?}", got("denied")));
        add("host-unreachable", got("host") == Some(false) && listener.accept().is_err(), format!("{}:{lport} -> {:?}", veth.host_ip, got("host")));
        let _ = child.wait();

        net_down_and_check(&mut add, &spec, forward_before, &before_forward, &veth, &table);
        let _ = std::fs::remove_file(spec_path);
        out
    }

    fn net_down_and_check(add: &mut dyn FnMut(&str, bool, String), spec: &crate::linuxapps_init::Spec, forward_before: Option<String>, before_forward: &str, veth: &Veth, table: &str) {
        crate::linuxapps_init::net_down(spec, forward_before);
        add("teardown-table", !table_there(table), table.to_string());
        add("teardown-veth", !interface_names().iter().any(|n| n.starts_with("ivh")), "no ivh interface left".into());
        let now = crate::sharing::real::ip_forward_get().unwrap_or_default();
        add("teardown-forwarding", now == before_forward, format!("before={before_forward} now={now}"));
        let _ = veth;
    }
