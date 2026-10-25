//! What the IYAKKI menu's Linux apps page asks the shell to do (child module of `shell`; `docs/LINUX-APPS.md`): read the facts, start the
//! detached jobs (creating a sandbox downloads and unpacks a base; apt installs packages), change a sandbox's switches and folders,
//! open the CLI tile inside a sandbox, and run or stop an image app in the dev cluster. Every change is written to the audit logs.

use super::*;
use crate::brandmenu::{BoxRow, ImageRow, LinuxAction, LinuxFacts};
use iyakki_agent::{imageapps, linuxapps, linuxapps_dist as dist, quickinstall as qi};
use std::sync::Mutex as StdMutex;

/// The image apps' states, asked of the cluster on a thread of its own (a few `kubectl` calls can take a while).
static STATES: StdMutex<Option<Vec<(String, String)>>> = StdMutex::new(None);
static STATES_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// What a cluster action (run, stop an image app) said when it finished: (text, good).
static ANSWERS: StdMutex<Vec<(String, bool)>> = StdMutex::new(Vec::new());
/// Sizes of the sandboxes on disk, worked out when the page opens or an action finishes (walking the files is not done every second).
static SIZES: StdMutex<Vec<(String, u64)>> = StdMutex::new(Vec::new());

impl Shell {
    fn linux_la(&self, user: &str) -> Result<linuxapps::LinuxApps, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
        Ok(linuxapps::LinuxApps::new(self.env.services.dir(), user, &exe, Arc::new(iyakki_agent::wsexec::System)))
    }

    fn kubectl_fn(svc: Arc<Services>) -> impl Fn(&[String]) -> iyakki_agent::wsexec::Out {
        move |a: &[String]| svc.k3sdev.kubectl(&a.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// The facts of the page: the files at once, the sandboxes' sizes from the last count, the image apps' states from the last answer.
    pub(super) fn linux_facts(&self, measure: bool) -> LinuxFacts {
        let Some(sess) = self.session.clone() else { return LinuxFacts::default() };
        let svc = self.env.services.clone();
        let la = match self.linux_la(&sess.user) {
            Ok(l) => l,
            Err(e) => return LinuxFacts { runtime: e, ..Default::default() },
        };
        if measure {
            *SIZES.lock().unwrap_or_else(|e| e.into_inner()) = la.list().iter().map(|s| (s.name.clone(), la.storage(&s.name))).collect();
        }
        let sizes = SIZES.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let boxes: Vec<BoxRow> = la
            .list()
            .into_iter()
            .map(|s| BoxRow { title: s.title(), network: s.network, mounts: s.mounts.iter().map(|m| (m.src.clone(), m.dst.clone(), m.rw)).collect(), mb: sizes.iter().find(|x| x.0 == s.name).map_or(0, |x| x.1 >> 20), name: s.name })
            .collect();
        let (apps, problems) = imageapps::catalog(&imageapps::dirs(svc.dir()));
        let states = STATES.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
        let images: Vec<ImageRow> = apps
            .iter()
            .map(|a| ImageRow { id: a.id.clone(), name: a.name.clone(), description: a.description.clone(), licence: a.licence.clone(), example: a.example, pinned: a.pinned(), image: a.image.clone(), status: states.iter().find(|x| x.0 == a.id).map(|x| x.1.clone()).unwrap_or_default() })
            .collect();
        let probe = linuxapps::Probe::read();
        let cluster = svc.k3sdev.installed().is_some();
        let runtime = match linuxapps::choose_runtime(&probe, cluster) {
            Ok(linuxapps::Runtime::Namespaced) => "Sandboxes run in Linux namespaces (own files, processes, network switch) with a system-call filter".to_string(),
            Ok(linuxapps::Runtime::K3sPod) => "This kernel lacks namespaces; the fallback (a pod in the dev cluster) is designed but not built yet".to_string(),
            Err(_) => String::new(),
        };
        let jobs_running = svc.jobs.visible(&sess.user, qi::WORKSPACE).iter().filter(|j| j.running() && linuxapps::is_linux_job(&j.command)).count();
        LinuxFacts {
            boxes,
            images,
            problems,
            kernel_missing: if probe.can_namespace() { vec![] } else { probe.missing() },
            optional_missing: if probe.can_namespace() { probe.optional_missing().into_iter().map(|m| m.split(':').next().unwrap_or("").split(" (").next().unwrap_or("").to_string()).collect() } else { vec![] },
            runtime,
            jobs_running,
            allowed: sess.policy.cli == iyakki_sec::policy::CliLevel::Full,
            any_host: sess.policy.net_hosts.iter().any(|h| h == "*"),
            cluster,
        }
    }

    /// The page opened or an action finished: read the facts and ask the cluster for the image apps' states.
    pub(super) fn refresh_linux(&mut self) {
        let f = self.linux_facts(true);
        let ask = f.cluster && !f.images.is_empty();
        self.brand.set_linux(f);
        if ask && !STATES_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let svc = self.env.services.clone();
            let dirs = imageapps::dirs(svc.dir());
            let _ = std::thread::Builder::new().name("iyakki-linux-states".into()).spawn(move || {
                let kc = Shell::kubectl_fn(svc);
                let (apps, _) = imageapps::catalog(&dirs);
                let v: Vec<(String, String)> = apps.iter().filter(|a| a.pinned()).map(|a| (a.id.clone(), imageapps::status(a, &kc))).collect();
                *STATES.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
                STATES_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
        self.stale = true;
    }

    /// Every half second or so while the page is open: answers of the cluster actions, new states, the jobs' count.
    pub(super) fn linux_poll(&mut self) {
        let answers: Vec<(String, bool)> = std::mem::take(&mut *ANSWERS.lock().unwrap_or_else(|e| e.into_inner()));
        let had = !answers.is_empty();
        if let Some((t, ok)) = answers.into_iter().last() {
            self.brand.note = Some((t, ok));
        }
        let f = self.linux_facts(false);
        if Some(&f) != self.brand.linux.facts.as_ref() || had {
            self.brand.set_linux(f);
            self.stale = true;
        }
    }

    fn linux_note(&mut self, t: impl Into<String>, good: bool) {
        self.brand.note = Some((t.into(), good));
    }

    fn linux_start_job(&mut self, user: &str, label: &str, args: &[String]) {
        let svc = self.env.services.clone();
        let launch = match self.quick_launch.clone() {
            Some(l) => Ok(l),
            None => qi::Launch::this_program(svc.dir()),
        };
        let launch = match launch {
            Ok(l) => l,
            Err(e) => return self.linux_note(e, false),
        };
        let _ = std::fs::create_dir_all(&launch.data);
        let spec = linuxapps::job_spec(&launch, user, label, args);
        match svc.jobs.start(user, &spec, &|_: &std::path::Path| Ok(())) {
            Ok(job) => {
                self.linux_note(format!("Started: {}. Progress is beside the clock; the log is in the installer runs.", job.label), true);
                if let Some(cx) = self.cx() {
                    self.installs.poll(&cx, true);
                }
            }
            Err(e) => self.linux_note(e, false),
        }
    }

    pub(super) fn linux_action(&mut self, a: LinuxAction) {
        let Some(sess) = self.session.clone() else { return };
        if matches!(a, LinuxAction::Refresh) {
            self.refresh_linux();
            return;
        }
        if sess.policy.cli != iyakki_sec::policy::CliLevel::Full {
            return self.linux_note("Linux apps need the full command-line permission for your account.", false);
        }
        let any_host = sess.policy.net_hosts.iter().any(|h| h == "*");
        let audit = |me: &Self, what: &str, d: &str| me.env.audit.record(&sess.user, what, d);
        let scope = |p: &std::path::Path, write: bool| if write { iyakki_agent::scope::check_write(&sess.policy, &sess.user, p) } else { iyakki_agent::scope::check_read(&sess.policy, &sess.user, p) };
        let la = match self.linux_la(&sess.user) {
            Ok(l) => l,
            Err(e) => return self.linux_note(e, false),
        };
        let svc = self.env.services.clone();
        match a {
            LinuxAction::Refresh => {}
            LinuxAction::Create { name, release, network, folder } => {
                if !linuxapps::valid_name(&name) || dist::release(&release).is_none() {
                    return self.linux_note("That name or release is not valid.", false);
                }
                if la.get(&name).is_ok() {
                    return self.linux_note(format!("A sandbox named {name} exists already."), false);
                }
                if network && !any_host {
                    return self.linux_note("A sandbox with its network on can reach any server; only an account allowed every host may make one.", false);
                }
                if !sess.policy.allows_host("registry-1.docker.io") || !sess.policy.allows_host("cdimage.ubuntu.com") {
                    return self.linux_note("Your account may not reach the distribution servers (cdimage.ubuntu.com, registry-1.docker.io).", false);
                }
                let mut args: Vec<String> = vec!["create".into(), name.clone(), release.clone(), "--net".into(), if network { "on" } else { "off" }.into()];
                if let Some((f, rw)) = &folder {
                    // checked now, with the person's file scope, so a refusal is shown here and not in a log
                    let canon = match linuxapps::mount_source_ok(f, svc.dir()) {
                        Ok(c) => c,
                        Err(e) => return self.linux_note(e, false),
                    };
                    if let Err(e) = scope(&canon, *rw) {
                        return self.linux_note(e, false);
                    }
                    args.push("--mount".into());
                    args.push(format!("{}{}", canon.display(), if *rw { ":rw" } else { "" }));
                }
                audit(self, "linuxapps.create", &format!("{name} {release} network={network}"));
                let title = dist::release(&release).map_or(release.clone(), |r| r.title.to_string());
                self.linux_start_job(&sess.user, &format!("Linux apps: create {name} ({title})"), &args);
            }
            LinuxAction::Install { name, packages } => self.linux_packages(&sess, &la, name, packages, false),
            LinuxAction::Remove { name, packages } => self.linux_packages(&sess, &la, name, packages, true),
            LinuxAction::Upgrade { name } => {
                if let Err(e) = la.get(&name) {
                    return self.linux_note(e, false);
                }
                if !any_host {
                    return self.linux_note("Upgrading downloads from the distribution through the sandbox's network, which reaches any server; only an account allowed every host may do that.", false);
                }
                audit(self, "linuxapps.upgrade", &name);
                self.linux_start_job(&sess.user, &format!("Linux apps: apt upgrade in {name}"), &["upgrade".into(), name]);
            }
            LinuxAction::SetNetwork { name, on } => {
                if on && !any_host {
                    return self.linux_note("A sandbox with its network on can reach any server; only an account allowed every host may turn it on.", false);
                }
                match la.set_network(&name, on) {
                    Ok(()) => {
                        audit(self, "linuxapps.network", &format!("{name} {}", if on { "on" } else { "off" }));
                        self.linux_note(format!("{name}: network {}.", if on { "on (it can reach any server)" } else { "off" }), true);
                        self.refresh_linux();
                    }
                    Err(e) => self.linux_note(e, false),
                }
            }
            LinuxAction::Mount { name, folder, rw } => match la.add_mount(&name, &folder, "", rw, &scope) {
                Ok(m) => {
                    audit(self, "linuxapps.mount", &format!("{name} {} {}", m.dst, if m.rw { "rw" } else { "ro" }));
                    self.linux_note(format!("{} is mounted at {} ({}).", m.src, m.dst, if m.rw { "read-write" } else { "read-only" }), true);
                    self.refresh_linux();
                }
                Err(e) => self.linux_note(e, false),
            },
            LinuxAction::Unmount { name, folder } => {
                let dst = match la.get(&name) {
                    Ok(s) => {
                        let canon = std::fs::canonicalize(&folder).map(|p| p.display().to_string()).unwrap_or_else(|_| folder.clone());
                        s.mounts.iter().find(|m| m.dst == folder || m.src == canon || m.src == folder).map(|m| m.dst.clone())
                    }
                    Err(e) => return self.linux_note(e, false),
                };
                match dst.ok_or_else(|| format!("{folder} is not mounted in {name}.")).and_then(|d| la.remove_mount(&name, &d).map(|_| d)) {
                    Ok(d) => {
                        audit(self, "linuxapps.unmount", &format!("{name} {d}"));
                        self.linux_note(format!("{d} is no longer mounted in {name}."), true);
                        self.refresh_linux();
                    }
                    Err(e) => self.linux_note(e, false),
                }
            }
            LinuxAction::Terminal { name } => match la.get(&name) {
                Ok(s) if s.network && !any_host => self.linux_note(format!("{name} has its network on, so it can reach any server; only an account allowed every host may open it. Turn its network off first."), false),
                Ok(_) => {
                    audit(self, "linuxapps.terminal", &name);
                    self.brand.close();
                    self.jump_to("cli", &format!("sandbox:{name}"));
                }
                Err(e) => self.linux_note(e, false),
            },
            LinuxAction::Delete { name, typed } => match la.delete(&name, &typed) {
                Ok(freed) => {
                    audit(self, "linuxapps.delete", &name);
                    self.linux_note(format!("{name} was deleted ({} MB freed).", freed >> 20), true);
                    self.refresh_linux();
                }
                Err(e) => self.linux_note(e, false),
            },
            LinuxAction::RunImage { id } => self.linux_image(&sess, id, false),
            LinuxAction::StopImage { id } => self.linux_image(&sess, id, true),
        }
    }

    fn linux_packages(&mut self, sess: &Session, la: &linuxapps::LinuxApps, name: String, packages: Vec<String>, remove: bool) {
        if let Err(e) = linuxapps::check_packages(&packages).and_then(|_| la.get(&name).map(|_| ())) {
            return self.linux_note(e, false);
        }
        if !sess.policy.net_hosts.iter().any(|h| h == "*") {
            return self.linux_note("Packages come from the distribution through the sandbox's network, which reaches any server; only an account allowed every host may install them.", false);
        }
        let verb = if remove { "remove" } else { "install" };
        self.env.audit.record(&sess.user, if remove { "linuxapps.apt.remove" } else { "linuxapps.apt.install" }, &format!("{name}: {}", packages.join(" ")));
        let mut args: Vec<String> = vec![verb.into(), name.clone()];
        args.extend(packages.iter().cloned());
        self.linux_start_job(&sess.user, &format!("Linux apps: {verb} {} in {name}", packages.join(" ")), &args);
    }

    /// Run or stop an image app in the dev cluster (managed `kubectl` only), on a thread of its own: the answer arrives with the next poll.
    fn linux_image(&mut self, sess: &Session, id: String, stop: bool) {
        let svc = self.env.services.clone();
        let (apps, _) = imageapps::catalog(&imageapps::dirs(svc.dir()));
        let Some(app) = apps.into_iter().find(|a| a.id == id) else { return self.linux_note(format!("There is no image app {id}."), false) };
        if svc.k3sdev.installed().is_none() {
            return self.linux_note("Image apps run in the dev cluster: install k3s first (Store \u{2192} Setup).", false);
        }
        self.env.audit.record(&sess.user, if stop { "imageapp.stop" } else { "imageapp.run" }, &format!("{} {}", app.id, app.image));
        self.linux_note(if stop { format!("Removing {} from the cluster...", app.name) } else { format!("Starting {} in the cluster...", app.name) }, true);
        let data = svc.dir().to_path_buf();
        let _ = std::thread::Builder::new().name("iyakki-image-app".into()).spawn(move || {
            let kc = Shell::kubectl_fn(svc);
            let r = if stop { imageapps::stop(&app, &kc) } else { imageapps::run(&app, &data, &kc) };
            let (t, ok) = match r {
                Ok(t) => (t, true),
                Err(e) => (e, false),
            };
            ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).push((t, ok));
            let st = imageapps::status(&app, &kc);
            let mut g = STATES.lock().unwrap_or_else(|e| e.into_inner());
            let v = g.get_or_insert_with(Vec::new);
            v.retain(|x| x.0 != app.id);
            v.push((app.id.clone(), st));
        });
    }
}
