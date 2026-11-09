//! Settings -> Domains (administrators): give services a name. The route table (host or wildcard, path prefix, target, http / https /
//! passthrough, WebSocket, who may reach it) with add, edit and remove, the validation words of the engine's own checker, the services
//! found on the local k3s (and this device's own) to fill a route's port from, the listener as a service (on or off, ports, status and the
//! last errors), the firewall opening as a step of its own, and the certificates kept in the vault. The logic is
//! `iyakki_agent::domains`; the model is explained in `docs/DOMAINS.md`.

use std::sync::Arc;

use iyakki_agent::cloudroute::{Access, CertRef, Route, Scheme, Target};
use iyakki_agent::domains::{self as dom, Found, ListenerChange, Manager, Own};

use super::*;

type FoundResult = Result<Vec<Found>, String>;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct DomPanel {
    /// Tests: a manager over fakes.
    pub(super) mgr: Option<Arc<Manager>>,
    pub(super) found: Option<FoundResult>,
    finding: Option<Receiver<FoundResult>>,
    asked: bool,
    /// The choices of the open route form, in the order of its "Send to" list, and of its certificate list.
    sends: Vec<(String, Target, Option<Scheme>, Option<String>)>,
    certs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DomForm {
    /// A new route, or the route with this host and path.
    Route(Option<(String, String)>),
    Listener,
    Firewall,
    Import,
}

impl SettingsApp {
    pub(super) fn dom_mgr(&self, cx: &AppCx) -> Arc<Manager> {
        if let Some(m) = &self.dom.mgr {
            return m.clone();
        }
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        Manager::shared(&cx.services, &cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))
    }

    pub(super) fn dom_len(&self, cx: &AppCx) -> usize {
        self.dom_mgr(cx).routes().len()
    }

    pub(super) fn dom_buttons(&self) -> Vec<Btn> {
        vec![("Add", ACCENT, 'a'), ("Edit", TEXT, 'e'), ("Delete", ERR, 'd'), ("Listener on / off", TEXT, 'x'), ("Ports", TEXT, 'p'), ("Firewall", TEXT, 'f'), ("Trial certificate", TEXT, 't'), ("Import certificate", TEXT, 'i'), ("Look again", TEXT, 'r')]
    }

    /// Looks for the cluster's services in the background (kubectl can be slow), and keeps the listener's checks going while the page is open.
    pub(super) fn dom_tick(&mut self, cx: &AppCx) {
        if self.tab() != Tab::Domains {
            return;
        }
        if !self.dom.asked {
            self.dom_look(cx);
        }
        if let Some(rx) = &self.dom.finding {
            if let Ok(r) = rx.try_recv() {
                self.dom.found = Some(r);
                self.dom.finding = None;
            }
        }
        if self.ticks % 40 == 0 {
            for line in self.dom_mgr(cx).tick() {
                self.ok(line);
            }
        }
    }

    fn dom_look(&mut self, cx: &AppCx) {
        self.dom.asked = true;
        let (tx, rx) = channel();
        let m = self.dom_mgr(cx);
        std::thread::spawn(move || {
            let _ = tx.send(m.discover());
        });
        self.dom.finding = Some(rx);
    }

    fn dom_row(&self, cx: &AppCx) -> Option<Route> {
        let rows = self.dom_mgr(cx).routes();
        let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
        rows.into_iter().nth(sel)
    }

    pub(super) fn handle_dom(&mut self, k: &Key, cx: &AppCx) {
        let mgr = self.dom_mgr(cx);
        let len = mgr.routes().len();
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('a') => self.dom_open_route(cx, None),
            Key::Char('e') => match self.dom_row(cx) {
                Some(r) => self.dom_open_route(cx, Some(r)),
                None => self.fail("Choose a route first (Up/Down)."),
            },
            Key::Char('d') | Key::Delete => match self.dom_row(cx) {
                Some(r) => self.modal = Some(Modal::Confirm(ConfirmKind::DomainDelete(r.host.clone(), r.path_prefix.clone()), format!("Remove the route {}{}? The listener stops serving the name at once. Certificates stay in your vault.", r.host, r.path_prefix))),
                None => self.fail("Choose a route first (Up/Down)."),
            },
            Key::Char('x') => {
                let on = !mgr.settings().enabled;
                match mgr.set_listener(ListenerChange { enabled: Some(on), ..Default::default() }) {
                    Ok(line) => self.ok(if on { format!("{line}. It listens on this device only until you open the firewall (f).") } else { "The listener is off: the ports are closed and the firewall opening is removed.".to_string() }),
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('p') => {
                let c = mgr.settings();
                self.open_form(
                    FormKind::Domains(DomForm::Listener),
                    Form::new("Ports of the domain listener")
                        .text("http port", &c.http_port.to_string(), "8080 (default); 1024 and up, or 80 after approval")
                        .text("https port", &c.https_port.to_string(), "8443 (default); 1024 and up, or 443 after approval")
                        .choice("I approve the standard ports 80 and 443 (they need the right to bind low ports)", &["No", "Yes, I approve"], 0),
                );
            }
            Key::Char('f') => {
                let c = mgr.settings();
                if !c.enabled {
                    self.fail("Switch the listener on first (x). Opening the firewall is a separate step.");
                    return;
                }
                self.open_form(
                    FormKind::Domains(DomForm::Firewall),
                    Form::new(&format!("Open ports {} and {} to the local network?", c.http_port, c.https_port))
                        .choice("Firewall opening (local network only, never the internet)", &["Closed", "Open to the local network"], usize::from(c.firewall))
                        .choice("Until", &["No end time", "1 hour", "8 hours", "24 hours"], 0),
                );
            }
            Key::Char('t') => match self.dom_row(cx) {
                Some(r) => match mgr.make_trial_cert(&r.host) {
                    Ok(name) => {
                        let mut msg = format!("A self-signed trial certificate \"{name}\" for {} is in your vault. Browsers do not trust it and show a warning.", r.host);
                        if r.scheme == Scheme::Https {
                            let mut r2 = r.clone();
                            r2.cert = Some(CertRef::Vault(name));
                            let ns = mgr.namespace_of(&r);
                            match mgr.save_route(Some((&r.host, &r.path_prefix)), r2, ns.as_deref()) {
                                Ok(()) => msg.push_str(" The route uses it now."),
                                Err(e) => msg.push_str(&format!(" The route could not take it: {e}")),
                            }
                        }
                        self.ok(msg);
                    }
                    Err(e) => self.fail(e),
                },
                None => self.fail("Choose a route first (Up/Down)."),
            },
            Key::Char('i') => self.open_form(
                FormKind::Domains(DomForm::Import),
                Form::new("Import a certificate you have")
                    .text("Name in the vault", "", "for example: my-store (lower case letters, digits, -)")
                    .text("Host it is for", self.dom_row(cx).map(|r| r.host).unwrap_or_default().as_str(), "store.example.org")
                    .text("Certificate file (PEM chain)", "", "/path/to/fullchain.pem")
                    .text("Private key file (PEM)", "", "/path/to/privkey.pem"),
            ),
            Key::Char('r') => {
                self.dom_look(cx);
                self.ok("Looking at the cluster again.");
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------------ the route form

    fn dom_open_route(&mut self, cx: &AppCx, edit: Option<Route>) {
        let mgr = self.dom_mgr(cx);
        let mut sends: Vec<(String, Target, Option<Scheme>, Option<String>)> = vec![("A port on this device (type it below)".into(), Target::Local { port: 0 }, None, None)];
        for Own { label, target, scheme, .. } in mgr.own() {
            sends.push((label, target, Some(scheme), None));
        }
        if let Some(Ok(found)) = &self.dom.found {
            for f in found.iter().filter(|f| f.namespace != "kube-system") {
                for p in &f.ports {
                    sends.push((f.label(p), f.target(p), None, Some(f.namespace.clone())));
                }
            }
        }
        let t = edit.clone().unwrap_or_else(|| Route { host: String::new(), path_prefix: "/".into(), target: Target::Local { port: 0 }, scheme: Scheme::Http, listen: None, websocket: true, redirect_http: false, cert: None, access: Access::Members, client_ca: None });
        let pick = sends.iter().position(|s| s.1 == t.target && s.1 != Target::Local { port: 0 }).unwrap_or(0);
        let mut certs: Vec<String> = vec![String::new(), "trial".into()];
        certs.extend(mgr.certs().unwrap_or_default().into_iter().map(|c| c.name));
        let cert_labels: Vec<String> = certs.iter().enumerate().map(|(i, c)| match i { 0 => "None (plain http or passthrough)".to_string(), 1 => "Make a trial self-signed certificate".to_string(), _ => format!("Vault certificate {c}") }).collect();
        let cert_refs: Vec<&str> = cert_labels.iter().map(String::as_str).collect();
        let cert_sel = match &t.cert { Some(CertRef::Vault(n)) => certs.iter().position(|c| c == n).unwrap_or(0), _ => 0 };
        let send_refs: Vec<&str> = sends.iter().map(|s| s.0.as_str()).collect();
        let port_text = match (&t.target, pick) { (Target::Local { port }, 0) if *port != 0 => port.to_string(), _ => String::new() };
        let services = match &self.dom.found {
            Some(Err(e)) => format!("Cluster services not listed: {e}"),
            None => "Looking at the cluster for services...".to_string(),
            _ => "Services found on the local cluster are in the list.".to_string(),
        };
        let f = Form::new(if edit.is_some() { "Edit domain route" } else { "Add a domain route" })
            .text("Name (host)", &t.host, "store.local, app.example.org or *.example.org")
            .text("Path prefix", &t.path_prefix, "/")
            .choice(&format!("Send to ({services})"), &send_refs, pick)
            .text("Port on this device (when the first choice is picked)", &port_text, "30580 is the private store")
            .choice("Served as", &["Plain http", "https with a certificate", "TLS passed through untouched"], match t.scheme { Scheme::Http => 0, Scheme::Https => 1, Scheme::Passthrough => 2 })
            .choice("Certificate (https only)", &cert_refs, cert_sel)
            .choice("WebSocket passes through", &["Yes", "No"], usize::from(!t.websocket))
            .choice("Who may reach it", &["People of this device (the listener cannot check; the firewall decides)", "Anyone who can reach the port"], usize::from(t.access == Access::Public));
        self.dom.sends = sends;
        self.dom.certs = certs;
        self.open_form(FormKind::Domains(DomForm::Route(edit.map(|r| (r.host, r.path_prefix)))), f);
    }

    pub(super) fn dom_submit(&mut self, cx: &AppCx, kind: &DomForm, form: &mut Form) -> Result<String, String> {
        let mgr = self.dom_mgr(cx);
        match kind {
            DomForm::Listener => {
                let port = |i: usize, what: &str| form.value(i).trim().parse::<u16>().map_err(|_| format!("The {what} port is a number from 1 to 65535"));
                mgr.set_listener(ListenerChange { http_port: Some(port(0, "http")?), https_port: Some(port(1, "https")?), standard_ports_approved: form.choice_idx(2) == 1, ..Default::default() }).map(|l| format!("Saved. {l}"))
            }
            DomForm::Firewall => {
                let open = form.choice_idx(0) == 1;
                let hours = [None, Some(1), Some(8), Some(24)][form.choice_idx(1).min(3)];
                mgr.set_listener(ListenerChange { firewall: Some(open), firewall_hours: hours, ..Default::default() }).map(|l| if open { format!("{l}. Other computers on the local network can reach the routes now. Switching the listener off closes it again.") } else { l })
            }
            DomForm::Import => {
                let read = |p: &str| std::fs::read_to_string(p.trim()).map_err(|e| format!("{}: {e}", p.trim()));
                let (chain, key) = (read(&form.value(2))?, read(&form.value(3))?);
                let line = mgr.import_cert(form.value(0).trim(), form.value(1).trim(), &chain, &key)?;
                Ok(format!("Kept in your vault as \"{}\": {line}. The key file you picked is still on disk: delete it when you no longer need it there.", form.value(0).trim()))
            }
            DomForm::Route(old) => {
                let pick = form.choice_idx(2).min(self.dom.sends.len().saturating_sub(1));
                let (_, target, own_scheme, ns) = self.dom.sends.get(pick).cloned().ok_or("Choose where it goes.")?;
                let target = if pick == 0 {
                    Target::Local { port: form.value(3).trim().parse::<u16>().map_err(|_| "Type the port on this device (a number from 1 to 65535), or pick a service in the list.".to_string())? }
                } else {
                    target
                };
                let mut scheme = match form.choice_idx(4) { 1 => Scheme::Https, 2 => Scheme::Passthrough, _ => Scheme::Http };
                if let Some(s @ Scheme::Passthrough) = own_scheme {
                    scheme = s;
                }
                let host = form.value(0).trim().to_ascii_lowercase();
                let cert = match (scheme, self.dom.certs.get(form.choice_idx(5)).cloned().unwrap_or_default()) {
                    (Scheme::Https, c) if c.is_empty() => return Err("An https route needs a certificate: pick a vault one or make a trial one.".into()),
                    (Scheme::Https, c) if c == "trial" => Some(CertRef::Vault(dom::cert_name_for(&host))),
                    (Scheme::Https, c) => Some(CertRef::Vault(c)),
                    _ => None,
                };
                let route = Route { host, path_prefix: form.value(1).trim().to_string(), target, scheme, listen: None, websocket: form.choice_idx(6) == 0, redirect_http: false, cert, access: if form.choice_idx(7) == 1 { Access::Public } else { Access::Members }, client_ca: None };
                if form.choice_idx(5) == 1 && scheme == Scheme::Https {
                    // the engine's checker first, so a refused route leaves no certificate behind
                    iyakki_agent::cloudroute::RouteTable { routes: vec![route.clone()], ingress_class: None }.check(&[])?;
                    mgr.make_trial_cert(&route.host)?;
                }
                let old = old.clone();
                mgr.save_route(old.as_ref().map(|(h, p)| (h.as_str(), p.as_str())), route.clone(), ns.as_deref())?;
                let st = mgr.status();
                Ok(format!(
                    "Saved {}{}. {}{}",
                    route.host,
                    route.path_prefix,
                    if st.running { "The listener serves it now." } else { "The listener is off: press x to switch it on." },
                    if route.host.ends_with(".local") { " The name ends in .local, so it is announced on the local network while the listener runs (Settings, Network: Announce this device must be on)." } else { "" }
                ))
            }
        }
    }

    pub(super) fn dom_delete(&mut self, cx: &AppCx, host: &str, path: &str) {
        match self.dom_mgr(cx).remove_route(host, path) {
            Ok(r) => {
                self.ok(format!("Removed {}{}.", r.host, r.path_prefix));
                let len = self.dom_len(cx);
                self.sel[self.tab] = self.sel[self.tab].min(len.saturating_sub(1));
            }
            Err(e) => self.fail(e),
        }
    }

    // ------------------------------------------------------------------------------------------------ drawing

    pub(super) fn render_dom(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mgr = self.dom_mgr(cx);
        let rows = mgr.routes();
        let st = mgr.status();
        let mut y = a.y;
        sshtun_page::intro(g, a, s, &mut y, &["Give a service a name: store.local instead of 192.168.1.20:30580. A name ending in .local is also announced on your network, so other computers find it. The listener starts on this device only; opening it to the local network is a separate step (f).", "Add a route (a): pick a service found on your k3s or on this device and its port is filled in. The listener (x) serves every route by the name in the browser's address."], "Names for services. Add a route (a), switch the listener on (x), then open the firewall (f) for other computers.", 170.0 * s + 52.0 * s);
        let col = if st.running { OK } else if st.enabled { ERR } else { DIM };
        let line = g.fit(14.0 * s, &format!("Listener: {}", st.line()), Face::Bold, a.w);
        g.text(a.x, y, 14.0 * s, col, &line, Face::Bold);
        y += 22.0 * s;
        if rows.is_empty() {
            for w in g.wrap(14.0 * s, "No routes yet. Press a to add one, for example store.local to the private store's port 30580.", Face::Sans, a.w) {
                g.text(a.x, y, 14.0 * s, DIM, &w, Face::Sans);
                y += 20.0 * s;
            }
        } else {
            let sel = self.sel[self.tab].min(rows.len() - 1);
            let detail_h = 150.0 * s;
            let rh = 52.0 * s;
            let room = (a.y + a.h - detail_h - y - 8.0 * s).max(rh);
            let heights = vec![rh; rows.len()];
            let first = self.win[self.tab].layout(sel, &heights, room);
            let fit = self.win[self.tab].fit();
            for (i, r) in rows.iter().enumerate().skip(first).take(fit) {
                let how = match r.scheme { Scheme::Http => "http", Scheme::Https => "https", Scheme::Passthrough => "passthrough" };
                let sub = format!("{} · {}{}", dom::target_text(&r.target), if r.websocket { "websocket · " } else { "" }, dom::access_text(r.access));
                let cert = mgr.cert_state(r);
                let mut chips: Vec<(&str, [u8; 4])> = vec![(how, MUTED)];
                if r.host.ends_with(".local") && st.running {
                    chips.push(("announced", OK));
                }
                if let Some(c) = &cert {
                    chips.push(if c.starts_with("valid for") { ("certificate ok", OK) } else { ("certificate", ERR) });
                }
                row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &format!("{}{}", r.host, r.path_prefix), &sub, &chips);
                y += rh;
            }
        }
        // the lower part: firewall, recent problems, certificates, what was found
        let mut lines: Vec<(String, [u8; 4])> = Vec::new();
        let cfg = mgr.settings();
        lines.push((if cfg.firewall { format!("Firewall: ports open to the local network only{}.", if cfg.firewall_until > 0 { " until the end time" } else { "" }) } else { "Firewall: nothing opened for the listener; other computers cannot reach it.".to_string() }, MUTED));
        for e in &st.last_errors {
            lines.push((format!("Problem: {e}"), ERR));
        }
        for (h, why) in &st.refused {
            lines.push((format!("{h} is not served over https: {why}"), ERR));
        }
        match mgr.certs() {
            Ok(c) if !c.is_empty() => lines.extend(c.iter().map(|r| (format!("Certificate {} (kept in your vault)", r.line()), if r.days <= 30 || r.problem.is_some() { crate::fx::AMBER } else { MUTED }))),
            Ok(_) => lines.push(("No certificates in the vault. t makes a trial one; i imports yours.".into(), DIM)),
            Err(e) => lines.push((format!("Certificates: {e}"), ERR)),
        }
        lines.push((
            match &self.dom.found {
                None => "Looking at the cluster for services...".to_string(),
                Some(Ok(f)) => format!("{} service(s) found on the local cluster; they are offered when you add a route.", f.iter().filter(|x| x.namespace != "kube-system").map(|x| x.ports.len()).sum::<usize>()),
                Some(Err(e)) => format!("Cluster services not listed: {e}"),
            },
            DIM,
        ));
        let mut ty = (a.y + a.h - 150.0 * s).max(y + 4.0 * s);
        g.rect(a.x, ty - 4.0 * s, a.w, 1.0, [255, 255, 255, 24]);
        'draw: for (l, c) in lines {
            for w in g.wrap(12.0 * s, &l, Face::Sans, a.w) {
                if ty + 16.0 * s > a.y + a.h {
                    break 'draw;
                }
                g.text(a.x, ty, 12.0 * s, c, &w, Face::Sans);
                ty += 16.0 * s;
            }
        }
    }
}
