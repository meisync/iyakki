//! The `linux` command of the CLI tile's terminal (child module of `cli`): Linux apps from the command line, and "open a terminal in a
//! sandbox" (`linux use <name>`: the prompt changes and every line you type runs inside the sandbox until `exit`).
//!
//! There is no shell anywhere: a line is split into words (quotes group, nothing is expanded) and the words are the sandbox
//! program's argument array. `bash -c '...'` inside the sandbox is the way to a pipe, on purpose and visible. Long work (creating a
//! sandbox, apt) runs as a detached job whose progress is beside the clock; short commands run here with a time limit.

use super::*;
use crate::linuxapps::{self, ExecOpts, LinuxApps};
use std::time::Duration;

/// How long a line typed in a sandbox terminal may run before it is stopped.
const LINE_SECONDS: u64 = 120;

pub(super) const HELP: &str = "linux · Debian/Ubuntu sandboxes (Linux apps). Subcommands:
  linux list                              your sandboxes
  linux releases                          what can be created
  linux new <name> <release> [net]        download a base and make a sandbox (a job; net = network on)
  linux use <name>                        a terminal inside it (lines run in the sandbox; exit leaves)
  linux run <name> <command...>           run one command (no shell: words only)
  linux apt install|remove <name> <package...>   (jobs), linux apt update|upgrade <name>
  linux apt search <name> <word...>, linux apt list <name>
  linux net <name> on|off · linux mount <name> <folder> [mount-point] [rw] · linux unmount <name> <mount-point>
  linux import <name> <tarball> · linux delete <name> <name again> · linux log";

impl Cli {
    fn linux_manager(&self) -> Result<(LinuxApps, Arc<crate::services::Services>), String> {
        let svc = self.services.clone().ok_or("Linux apps are not available here")?;
        if self.policy.cli != CliLevel::Full {
            return Err("not permitted for your role".into());
        }
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        Ok((LinuxApps::new(svc.dir(), &self.user, &exe, Arc::new(crate::wsexec::System)), svc))
    }

    /// A sandbox with its network on can reach any server, so only an account that may reach every host runs it.
    fn linux_net_gate(&self, la: &LinuxApps, name: &str) -> Result<(), String> {
        let s = la.get(name)?;
        if s.network && !self.policy.net_hosts.iter().any(|h| h == "*") {
            return Err(format!("{name} has its network on, so it can reach any server; only an account allowed every host may run it. Turn the network off with `linux net {name} off`"));
        }
        Ok(())
    }

    fn linux_scope(&self) -> impl Fn(&Path, bool) -> Result<(), String> + '_ {
        move |p, write| if write { crate::scope::check_write(&self.policy, &self.user, p) } else { crate::scope::check_read(&self.policy, &self.user, p) }
    }

    fn linux_job(&self, svc: &crate::services::Services, label: &str, args: &[String]) -> Result<CliOut, String> {
        let l = crate::quickinstall::Launch::this_program(svc.dir())?;
        let spec = linuxapps::job_spec(&l, &self.user, label, args);
        let job = svc.jobs.start(&self.user, &spec, &|_: &Path| Ok(()))?;
        Ok(out(format!("started: {} (job {}). Its progress is beside the clock in the top bar; the log is in the installer list.", job.label, job.id)))
    }

    pub(super) fn linux_cmd(&mut self, rest: &str) -> Result<CliOut, String> {
        let words = crate::exec::split_words(rest)?;
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let (la, svc) = self.linux_manager()?;
        match w.as_slice() {
            [] | ["help"] => Ok(out(HELP)),
            ["list"] => {
                let all = la.list();
                if all.is_empty() {
                    return Ok(out("no sandboxes yet: `linux new <name> <release>` (releases: linux releases)"));
                }
                Ok(out(all
                    .iter()
                    .map(|s| format!("{:<14} {:<26} network {:<3} {} folder(s) mounted · {} MB", s.name, s.title(), if s.network { "on" } else { "off" }, s.mounts.len(), la.storage(&s.name) >> 20))
                    .collect::<Vec<_>>()
                    .join("\n")))
            }
            ["releases"] => Ok(out(crate::linuxapps_dist::RELEASES.iter().map(|r| format!("{:<14} {}", r.id, r.title)).collect::<Vec<_>>().join("\n") + "\n" + crate::linuxapps_dist::LICENCE_NOTE)),
            ["new", name, release, opts @ ..] => {
                let net = opts.contains(&"net");
                if opts.iter().any(|o| *o != "net") {
                    return Err("usage: linux new <name> <release> [net]".into());
                }
                // refuse a bad name or release now rather than in the job's log
                if !linuxapps::valid_name(name) || crate::linuxapps_dist::release(release).is_none() {
                    return Err("usage: linux new <name> <release> (names: lower-case letters, digits, -; releases: linux releases)".into());
                }
                if net && !self.policy.net_hosts.iter().any(|h| h == "*") {
                    return Err("a sandbox with its network on can reach any server; only an account allowed every host may make one. Make it without `net`".into());
                }
                if !self.policy.allows_host("registry-1.docker.io") || !self.policy.allows_host("cdimage.ubuntu.com") {
                    return Err("your account may not reach the distribution servers (cdimage.ubuntu.com, registry-1.docker.io)".into());
                }
                let mut a: Vec<String> = vec!["create".into(), (*name).into(), (*release).into()];
                if net {
                    a.extend(["--net".into(), "on".into()]);
                }
                self.linux_job(&svc, &format!("Linux apps: create {name}"), &a)
            }
            ["use", "off"] => {
                self.sandbox = None;
                Ok(out("back in the IYAKKI terminal"))
            }
            ["use", name] => {
                la.get(name)?;
                self.linux_net_gate(&la, name)?;
                self.sandbox = Some((*name).to_string());
                la.get(name).map(|s| out(format!("inside {} ({}). Lines run there as the sandbox user, one command each, no shell. `exit` leaves. Network: {}.", s.name, s.title(), if s.network { "on" } else { "off" })))
            }
            ["run", name, cmd @ ..] if !cmd.is_empty() => {
                self.linux_net_gate(&la, name)?;
                let o = la.exec(name, &cmd.iter().map(|c| c.to_string()).collect::<Vec<_>>(), &ExecOpts { timeout: Duration::from_secs(LINE_SECONDS), ..Default::default() })?;
                Ok(out(sandbox_text(&o)))
            }
            ["apt", "install" | "remove", name, pkgs @ ..] => {
                let pkgs: Vec<String> = pkgs.iter().map(|p| p.to_string()).collect();
                linuxapps::check_packages(&pkgs)?;
                la.get(name)?;
                self.linux_net_gate(&la, name)?;
                let verb = if w[1] == "install" { "install" } else { "remove" };
                let mut a = vec![verb.to_string(), (*name).to_string()];
                a.extend(pkgs.iter().cloned());
                self.linux_job(&svc, &format!("Linux apps: {verb} {} in {name}", pkgs.join(" ")), &a)
            }
            ["apt", verb @ ("update" | "upgrade"), name] => {
                la.get(name)?;
                self.linux_net_gate(&la, name)?;
                self.linux_job(&svc, &format!("Linux apps: apt {verb} in {name}"), &[verb.to_string(), (*name).to_string()])
            }
            ["apt", "search", name, words @ ..] => {
                self.linux_net_gate(&la, name)?;
                let r = la.apt_search(name, &words.iter().map(|x| x.to_string()).collect::<Vec<_>>())?;
                Ok(out(if r.is_empty() { "nothing found (run `linux apt update` first if the package lists are empty)".to_string() } else { r.iter().map(|(a, b)| format!("{a} - {b}")).collect::<Vec<_>>().join("\n") }))
            }
            ["apt", "list", name] => {
                self.linux_net_gate(&la, name)?;
                let r = la.installed(name)?;
                Ok(out(format!("{} package(s)\n{}", r.len(), r.iter().map(|(a, b)| format!("{a} {b}")).collect::<Vec<_>>().join("\n"))))
            }
            ["net", name, state @ ("on" | "off")] => {
                if *state == "on" && !self.policy.net_hosts.iter().any(|h| h == "*") {
                    return Err("a sandbox with its network on can reach any server; only an account allowed every host may turn it on".into());
                }
                la.set_network(name, *state == "on")?;
                Ok(out(format!("{name}: network {state}")))
            }
            ["mount", name, folder, more @ ..] => {
                let rw = more.contains(&"rw");
                let dst = more.iter().find(|m| m.starts_with('/')).copied().unwrap_or("");
                let scope = self.linux_scope();
                let folder = self.resolve(folder)?.display().to_string();
                let m = la.add_mount(name, &folder, dst, rw, &scope)?;
                Ok(out(format!("{} is mounted at {} ({})", m.src, m.dst, if m.rw { "read-write" } else { "read-only" })))
            }
            ["unmount", name, dst] => {
                la.remove_mount(name, dst)?;
                Ok(out(format!("{dst} is no longer mounted in {name}")))
            }
            ["import", name, tarball] => {
                let p = self.resolve(tarball)?;
                self.readable(&p)?;
                self.linux_job(&svc, &format!("Linux apps: import {name}"), &["import".into(), (*name).into(), p.display().to_string()])
            }
            ["delete", name, typed] => {
                let freed = la.delete(name, typed)?;
                if self.sandbox.as_deref() == Some(name) {
                    self.sandbox = None;
                }
                Ok(out(format!("{name} was deleted ({} MB freed)", freed >> 20)))
            }
            ["log"] => Ok(out(la.audit_tail(30).iter().map(|(t, u, a, d)| format!("{t} {u} {a} {d}")).collect::<Vec<_>>().join("\n"))),
            _ => Err("unknown subcommand; `linux help` lists them".into()),
        }
    }

    /// A line typed while inside a sandbox (`linux use <name>`).
    pub(super) fn sandbox_line(&mut self, name: &str, line: &str) -> CliOut {
        let run = || -> Result<String, String> {
            let (la, _) = self.linux_manager()?;
            self.linux_net_gate(&la, name)?;
            let o = la.run_line(name, line, None)?;
            Ok(sandbox_text(&o))
        };
        match run() {
            Ok(t) => out(t),
            Err(e) => out(format!("{name}: {e}")),
        }
    }
}

fn sandbox_text(o: &crate::wsexec::Out) -> String {
    let mut t = o.combined();
    if o.timed_out {
        t.push_str(&format!("\n(stopped after {LINE_SECONDS} seconds; use a job for long work)"));
    } else if o.missing {
        t.push_str("\n(the IYAKKI program could not be started)");
    } else if let Some(c) = o.code.filter(|c| *c != 0) {
        t.push_str(&format!("\n(exit status {c})"));
    }
    if o.truncated {
        t.push_str("\n(output cut)");
    }
    t.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::tests::MockHost;
    use iyakki_sec::policy::Role;

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn setup(tag: &str, net: bool) -> (Cli, Arc<crate::services::Services>, PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-clilinux-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let svc = crate::services::Services::open(&d, Arc::new(Off), Arc::new(Off)).unwrap();
        let mut p = Role::Admin.template();
        p.net_hosts = if net { vec!["*".into()] } else { vec!["example.com".into()] };
        let c = Cli::new("alice", p, Arc::new(MockHost::default())).with_services(svc.clone());
        // a sandbox record by hand (no download in a unit test)
        let sdir = d.join("linuxapps/alice/dev");
        std::fs::create_dir_all(sdir.join("rootfs")).unwrap();
        let s = linuxapps::Sandbox { name: "dev".into(), owner: "alice".into(), release: "debian-12".into(), created: 1, network: true, mounts: vec![], id_base: 0, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: "x".into(), source: "x".into(), arch: "amd64".into() };
        std::fs::write(sdir.join("sandbox.json"), serde_json::to_string(&s).unwrap()).unwrap();
        (c, svc, d)
    }

    #[test]
    fn the_linux_command_lists_gates_the_network_and_switches_the_prompt() {
        let (mut c, _svc, d) = setup("gate", false);
        assert!(c.exec("linux list").text.contains("dev") && c.exec("linux list").text.contains("Debian 12"));
        assert!(c.exec("linux releases").text.contains("ubuntu-24.04") && c.exec("linux releases").text.contains("trademarks"));
        // a sandbox with its network on needs an account that may reach every host
        let t = c.exec("linux use dev").text;
        assert!(t.contains("can reach any server") && c.sandbox.is_none(), "{t}");
        assert!(c.exec("linux run dev ls").text.contains("can reach any server"));
        assert!(c.exec("linux apt install dev curl").text.contains("can reach any server"));
        assert!(c.exec("linux net dev on").text.contains("may turn it on"));
        // turn it off: now it can be entered, the prompt changes and `exit` leaves
        assert!(c.exec("linux net dev off").text.contains("network off"));
        assert!(c.exec("linux use dev").text.contains("inside dev"));
        assert_eq!(c.prompt(), "alice@dev:~$ ");
        assert!(c.exec("exit").text.contains("left the sandbox"));
        assert!(c.prompt().contains("@iyakki:"));
        // names and packages are checked before a job is made
        assert!(c.exec("linux apt install dev -y").text.contains("not a package name"));
        assert!(c.exec("linux new Bad ubuntu-24.04").text.contains("usage"));
        assert!(c.exec("linux new okname arch").text.contains("usage"));
        assert!(c.exec("linux new okname ubuntu-24.04 net").text.contains("can reach any server"), "{}", c.exec("linux new okname ubuntu-24.04 net").text);
        // delete needs the name typed
        assert!(c.exec("linux delete dev nope").text.contains("type the name"));
        assert!(d.join("linuxapps/alice/dev").exists());
        assert!(c.exec("linux delete dev dev").text.contains("deleted"));
        assert!(!d.join("linuxapps/alice/dev").exists());
        assert!(c.exec("linux log").text.contains("sandbox.delete"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_a_full_terminal_has_the_linux_command() {
        let (_, svc, d) = setup("role", true);
        let mut user = Cli::new("alice", Role::User.template(), Arc::new(MockHost::default())).with_services(svc);
        assert!(user.exec("linux list").text.contains("not permitted"));
        assert!(!user.exec("help").text.contains("linux"));
        let (mut admin, _, d2) = setup("role2", true);
        assert!(admin.exec("help").text.contains("linux"));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn a_new_sandbox_and_apt_are_detached_jobs_with_the_argument_array_on_the_command_line() {
        let (mut c, svc, d) = setup("job", true);
        // the distribution hosts are reachable for this account (net_hosts has *)
        let t = c.exec("linux new work debian-12").text;
        assert!(t.starts_with("started: Linux apps: create work"), "{t}");
        let t = c.exec("linux apt install dev curl git").text;
        assert!(t.starts_with("started:"), "{t}");
        let jobs = svc.jobs.list("alice");
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| crate::linuxapps::is_linux_job(&j.command) && j.workspace == crate::quickinstall::WORKSPACE), "{jobs:?}");
        assert!(jobs.iter().any(|j| j.command.ends_with("install dev curl git")), "{jobs:?}");
        for j in &jobs {
            let _ = svc.jobs.stop("alice", &j.id, Duration::from_millis(100));
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
