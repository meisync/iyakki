//! K8S: "Launch cloud mode" on the selected cluster (docs/development/CLOUD-MULTITENANT-PLAN.md, step 2). A card with the tenant, the image,
//! the vault choice and an optional registry to push to; Preview shows exactly what would be applied (and what blocks it), Launch applies it
//! to the cluster this tile is working against, local or remote. The plan, the checks and the manifests are `iyakki_agent::cloudlaunch`.

use std::sync::{Arc, Mutex};

use iyakki_agent::cloudlaunch::{self as cl, ClusterRef, ImageStep, Launch, Plan, VaultPlan};
use iyakki_agent::cloudroute::{CertRef, Route, RouteTable, Scheme, Target};
use iyakki_agent::cloudtenant::TenantConfig;

use super::k8s_tile::*;
use super::{AppCx, Flow};
use crate::fx::{self, AMBER};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

pub(super) const T_CLOUD: usize = 67;
pub(super) const T_CLOUD_PREVIEW: usize = 68;
pub(super) const T_CLOUD_GO: usize = 69;
pub(super) const T_CLOUD_VAULT: usize = 70;
pub(super) const T_CLOUD_CLOSE: usize = 71;
/// 72..80: the fields (tenant, image, vault address, registry; then the route's domain, service, port and certificate name).
pub(super) const T_CLOUD_FIELD: usize = 72;
/// The Routes page: open it, go back, the http/https switch, the certificate source switch, add the typed route, remove route n.
pub(super) const T_CLOUD_ROUTES: usize = 80;
pub(super) const T_CLOUD_DONE: usize = 81;
pub(super) const T_CLOUD_SCHEME: usize = 82;
pub(super) const T_CLOUD_CERTSRC: usize = 83;
pub(super) const T_CLOUD_ADD: usize = 84;
/// 85..89: remove the nth listed route.
pub(super) const T_CLOUD_RM: usize = 85;
const RM_SHOWN: usize = 4;
/// The last id the Launch cloud mode card uses.
pub(super) const T_CLOUD_LAST: usize = T_CLOUD_RM + RM_SHOWN - 1;
/// The most routes one launch takes from this form.
const MAX_ROUTES: usize = 12;

const FIELDS: usize = 8;
/// The first field of the Routes page.
const ROUTE_FIELDS: usize = 4;

pub(super) struct CloudForm {
    pub fields: [TextField; FIELDS],
    pub focus: usize,
    /// 0 no vault service, 1 a remote vault address, 2 this cluster's own vault service.
    pub vault: u8,
    pub shown: Option<(Launch, Plan)>,
    pub text: Vec<String>,
    pub lines: Arc<Mutex<Vec<String>>>,
    pub done: Arc<Mutex<Option<Result<(), String>>>>,
    pub running: bool,
    reported: bool,
    /// The Routes page is shown instead of the launch fields.
    pub routes_page: bool,
    /// The routes added so far; the launch renders them (`Launch::routes`).
    pub routes: Vec<Route>,
    /// The route being typed: https (else http), and whether its certificate is a vault name (else a Secret made from the certificate files).
    pub https: bool,
    pub cert_vault: bool,
    /// What is wrong with the route typed (never contains what was typed in the certificate field).
    pub route_error: Option<String>,
}

impl CloudForm {
    pub(super) fn new() -> CloudForm {
        let mut image = TextField::new("image the cluster runs", false);
        image.set(&TenantConfig::example("x").image);
        CloudForm {
            fields: [TextField::new("tenant name, e.g. acme", false), image, TextField::new("https://vault.example.org", false), TextField::new("push the image to host:port/repo:tag (optional)", false), TextField::new("app.example.org", false), TextField::new("service, e.g. iyakki", false), TextField::new("port", false), TextField::new("name of the Secret or vault entry", false)],
            focus: 0,
            vault: 0,
            shown: None,
            text: Vec::new(),
            lines: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(Mutex::new(None)),
            running: false,
            reported: false,
            routes_page: false,
            routes: Vec::new(),
            https: false,
            cert_vault: false,
            route_error: None,
        }
    }

    fn visible(&self, i: usize) -> bool {
        if self.routes_page {
            i >= ROUTE_FIELDS && (i != 7 || self.https)
        } else {
            i < ROUTE_FIELDS && (i != 2 || self.vault == 1)
        }
    }

    fn first_visible(&self) -> usize {
        (0..FIELDS).find(|i| self.visible(*i)).unwrap_or(0)
    }

    fn last_visible(&self) -> usize {
        (0..FIELDS).rev().find(|i| self.visible(*i)).unwrap_or(0)
    }

    /// The route typed on the Routes page, checked with the validators of `cloudroute` (and added to the others to see that they work together).
    fn route_typed(&self) -> Result<Route, String> {
        let host = self.fields[4].value.trim().to_ascii_lowercase();
        let service = self.fields[5].value.trim().to_string();
        let port: u16 = self.fields[6].value.trim().parse().map_err(|_| "The port is a number from 1 to 65535.".to_string())?;
        let cert = self.fields[7].value.trim().to_string();
        let cert_ref = if self.https {
            // a name, never the certificate or its key: a pasted PEM is refused without being echoed
            if cert.is_empty() {
                return Err("An https route needs a certificate: type the name of the Secret (made from your certificate files) or of the vault entry.".into());
            }
            if cert.len() > 200 || cert.contains("BEGIN") || cert.chars().any(char::is_whitespace) {
                return Err("The certificate is given by name (a Secret or a vault entry), not pasted here.".into());
            }
            Some(if self.cert_vault { CertRef::Vault(cert) } else { CertRef::Secret(cert) })
        } else {
            None
        };
        if self.routes.len() >= MAX_ROUTES {
            return Err(format!("This form takes up to {MAX_ROUTES} routes."));
        }
        let route = Route {
            host,
            path_prefix: "/".into(),
            target: Target::Service { name: service, port },
            scheme: if self.https { Scheme::Https } else { Scheme::Http },
            listen: None,
            websocket: true,
            redirect_http: false,
            cert: cert_ref,
            access: Default::default(),
        };
        let mut all = self.routes.clone();
        all.push(route.clone());
        RouteTable { routes: all, ingress_class: None }.check(&[])?;
        Ok(route)
    }

    /// Anything typed on the Routes page and not added yet.
    fn route_pending(&self) -> bool {
        self.fields[4..7].iter().any(|f| !f.value.trim().is_empty())
    }

    fn step(&mut self, back: bool) {
        for _ in 0..FIELDS {
            self.focus = if back { (self.focus + FIELDS - 1) % FIELDS } else { (self.focus + 1) % FIELDS };
            if self.visible(self.focus) {
                return;
            }
        }
    }

    fn vault_label(&self) -> &'static str {
        match self.vault {
            0 => "Vault: none (the vaults stay on the tenant's volume)",
            1 => "Vault: a remote vault service (address below)",
            _ => "Vault: create one in this cluster (not available yet)",
        }
    }
}

impl K8sApp {
    pub(super) fn open_cloud(&mut self) {
        if !self.admin {
            self.say("Only administrators can launch cloud mode.", false);
            return;
        }
        self.cloud = Some(CloudForm::new());
    }

    /// What the form asks for, as a launch on the cluster this tile works against.
    fn cloud_launch(&self) -> Option<Launch> {
        let f = self.cloud.as_ref()?;
        let mut tenant = TenantConfig::example(f.fields[0].value.trim());
        let push = f.fields[3].value.trim().to_string();
        let image = f.fields[1].value.trim();
        if !image.is_empty() {
            tenant.image = image.to_string();
        }
        let image_step = if push.is_empty() {
            ImageStep::Existing
        } else {
            tenant.image = push.clone();
            ImageStep::Push { dest: push }
        };
        let vault = match f.vault {
            0 => VaultPlan::None,
            1 => VaultPlan::Remote(f.fields[2].value.trim().to_string()),
            _ => VaultPlan::Own,
        };
        let read_only = self.change_block().or_else(|| (!self.allow_changes()).then(|| "turn on Allow changes in this tile first".to_string()));
        let cluster = ClusterRef { name: self.entry().label.clone(), looks_like_production: false, read_only, changes_allowed: true };
        let routes = RouteTable { routes: f.routes.clone(), ingress_class: None };
        Some(Launch { tenant, vault, routes, image: image_step, cluster })
    }

    fn cloud_add_route(&mut self) {
        let Some(f) = &mut self.cloud else { return };
        match f.route_typed() {
            Ok(r) => {
                f.routes.push(r);
                for i in 4..FIELDS {
                    f.fields[i].clear();
                }
                f.route_error = None;
                f.focus = f.first_visible();
                f.shown = None;
            }
            Err(e) => f.route_error = Some(e),
        }
    }

    fn cloud_preview(&mut self) {
        if let Some(f) = &mut self.cloud {
            if f.route_pending() {
                f.routes_page = true;
                f.focus = f.first_visible();
                f.route_error = Some("You typed a route and did not add it: press Add route, or clear the fields.".into());
                return;
            }
        }
        let Some(l) = self.cloud_launch() else { return };
        let p = cl::plan(&l);
        let text = cl::preview(&l, &p);
        if let Some(f) = &mut self.cloud {
            f.text = text.lines().map(str::to_string).collect();
            f.shown = Some((l, p));
        }
    }

    fn cloud_go(&mut self, cx: &AppCx) {
        let (kube, tg, work, phrase, user) = (self.kube(), self.entry().target.clone(), self.dir.join("cloud-launch"), self.phrase(), self.user.clone());
        let Some(f) = &mut self.cloud else { return };
        if f.running {
            return;
        }
        let Some((l, p)) = f.shown.clone() else {
            self.say("Preview first: Launch applies exactly what the preview shows.", false);
            return;
        };
        if !p.blockers.is_empty() {
            self.say("The preview shows what blocks the launch.", false);
            return;
        }
        let Some(f) = &mut self.cloud else { return };
        cx.audit.record(&user, "k8s.cloud.launch", &format!("tenant={} routes={} on {phrase}", l.tenant.tenant, l.routes.routes.len()));
        let (lines, done) = (f.lines.clone(), f.done.clone());
        if let Ok(mut v) = lines.lock() {
            v.clear();
        }
        *done.lock().unwrap_or_else(|e| e.into_inner()) = None;
        f.running = true;
        f.reported = false;
        std::thread::spawn(move || {
            let say = |t: &str| lines.lock().unwrap_or_else(|e| e.into_inner()).extend(t.lines().map(str::to_string));
            let mut kubectl = |a: &[String]| {
                let mut args = tg.base_args_secs(60);
                args.extend(a.iter().cloned());
                say(&format!("$ kubectl {}", a.join(" ")));
                let o = kube.run_with(&args, 90);
                if o.missing {
                    return Err("kubectl was not found".to_string());
                }
                if o.ok() {
                    Ok(o.combined())
                } else {
                    let t = o.combined();
                    Err(t.trim().lines().last().unwrap_or("kubectl failed").to_string())
                }
            };
            let mut push = |dest: &str| {
                let mut out = Vec::new();
                iyakki_agent::cloudcli::run(&["image".into(), "--push".into(), dest.to_string(), "--name".into(), dest.to_string()], &mut |l| out.push(l.to_string()))?;
                Ok(out.join("\n"))
            };
            let r = cl::execute(&l, &p, &work, &mut kubectl, &mut push);
            let res = match r {
                Ok(v) => {
                    for t in v {
                        say(&t);
                    }
                    Ok(())
                }
                Err(e) => {
                    say(&e);
                    Err(e)
                }
            };
            *done.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
        });
    }

    pub(super) fn cloud_key(&mut self, k: &Key, _cx: &AppCx) -> Flow {
        let Some(f) = &mut self.cloud else { return Flow::Stay };
        match k {
            Key::Esc if !f.running && f.routes_page => {
                f.routes_page = false;
                f.focus = f.first_visible();
            }
            Key::Esc if !f.running => self.cloud = None,
            Key::Tab | Key::Down => f.step(false),
            Key::BackTab | Key::Up => f.step(true),
            Key::Enter if f.routes_page => {
                if f.focus == f.last_visible() {
                    self.cloud_add_route();
                } else {
                    f.step(false);
                }
            }
            Key::Enter => self.cloud_preview(),
            other => {
                if !f.running {
                    let i = f.focus;
                    f.fields[i].handle(other);
                    f.route_error = None;
                    f.shown = None; // what is shown no longer matches the form
                }
            }
        }
        Flow::Stay
    }

    pub(super) fn cloud_click(&mut self, n: usize, cx: &AppCx) {
        match n {
            T_CLOUD => self.open_cloud(),
            T_CLOUD_PREVIEW => self.cloud_preview(),
            T_CLOUD_GO => self.cloud_go(cx),
            T_CLOUD_CLOSE => {
                if self.cloud.as_ref().is_some_and(|f| !f.running) {
                    self.cloud = None;
                }
            }
            T_CLOUD_ROUTES => {
                if let Some(f) = self.cloud.as_mut().filter(|f| !f.running) {
                    f.routes_page = true;
                    f.focus = f.first_visible();
                }
            }
            T_CLOUD_DONE => {
                if let Some(f) = &mut self.cloud {
                    f.routes_page = false;
                    f.focus = f.first_visible();
                }
            }
            T_CLOUD_SCHEME | T_CLOUD_CERTSRC | T_CLOUD_ADD => {
                if let Some(f) = self.cloud.as_mut().filter(|f| !f.running && f.routes_page) {
                    f.route_error = None;
                    match n {
                        T_CLOUD_SCHEME => {
                            f.https = !f.https;
                            if !f.https && f.focus == 7 {
                                f.focus = 6;
                            }
                        }
                        T_CLOUD_CERTSRC => f.cert_vault = !f.cert_vault,
                        _ => self.cloud_add_route(),
                    }
                    if let Some(f) = &mut self.cloud {
                        f.shown = None;
                    }
                }
            }
            n if (T_CLOUD_RM..T_CLOUD_RM + RM_SHOWN).contains(&n) => {
                if let Some(f) = self.cloud.as_mut().filter(|f| !f.running) {
                    let i = n - T_CLOUD_RM;
                    if i < f.routes.len() {
                        f.routes.remove(i);
                        f.shown = None;
                    }
                }
            }
            T_CLOUD_VAULT => {
                if let Some(f) = &mut self.cloud {
                    if !f.running {
                        f.vault = (f.vault + 1) % 3;
                        if f.focus == 2 && f.vault != 1 {
                            f.focus = 0;
                        }
                        f.shown = None;
                    }
                }
            }
            n if (T_CLOUD_FIELD..T_CLOUD_FIELD + FIELDS).contains(&n) => {
                if let Some(f) = &mut self.cloud {
                    if f.visible(n - T_CLOUD_FIELD) {
                        f.focus = n - T_CLOUD_FIELD;
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn draw_cloud(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // the launch ended: say so once, and put it in the audit trail
        let cluster = self.entry().label.clone();
        let mut said: Option<(String, bool)> = None;
        if let Some(f) = &mut self.cloud {
            if f.running && !f.reported {
                if let Some(r) = f.done.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    f.running = false;
                    f.reported = true;
                    said = Some(match r {
                        Ok(()) => (format!("Cloud mode launched on {cluster}."), true),
                        Err(e) => (format!("Launch stopped: {e}"), false),
                    });
                }
            }
        }
        if let Some((m, ok)) = said {
            cx.audit.record(&self.user, if ok { "k8s.cloud.launched" } else { "k8s.cloud.failed" }, &m);
            self.say(m, ok);
        }
        let label = self.entry().label.clone();
        let Some(f) = &mut self.cloud else { return };
        let p = fx::modal(g, area, s, (area.w - 20.0 * s).min(820.0 * s), (area.h - 20.0 * s).min(640.0 * s), AMBER, false);
        let title = format!("Launch cloud mode on {label}");
        let mut y = fx::modal_title(g, p, s, &title, AMBER) + 4.0 * s;
        let lw = 150.0 * s;
        if f.routes_page {
            draw_routes_page(g, p, s, y, f, tick);
            return;
        }
        let field = |g: &mut Gfx, f: &CloudForm, y: f32, name: &str, i: usize, tick: u64| {
            g.text(p.x + 26.0 * s, y + 5.0 * s, 13.0 * s, MUTED, name, Face::Sans);
            f.fields[i].render(g, R::new(p.x + 26.0 * s + lw, y, p.w - 52.0 * s - lw, 28.0 * s), f.focus == i && !f.running, s, super::blink(tick), Face::Sans, T_CLOUD_FIELD + i);
        };
        field(g, f, y, "Tenant", 0, tick);
        y += 34.0 * s;
        field(g, f, y, "Image", 1, tick);
        y += 34.0 * s;
        g.button(p.x + 26.0 * s, y + 2.0 * s, 13.0 * s, f.vault_label(), MUTED, Event::Tab(T_CLOUD_VAULT));
        y += 34.0 * s;
        if f.vault == 1 {
            field(g, f, y, "Vault address", 2, tick);
            y += 34.0 * s;
        }
        field(g, f, y, "Push to registry", 3, tick);
        y += 34.0 * s;
        let rl = match f.routes.len() {
            0 => "Routes: none (add domain names that reach a service)".to_string(),
            1 => "Routes: 1".to_string(),
            n => format!("Routes: {n}"),
        };
        g.button(p.x + 26.0 * s, y + 2.0 * s, 13.0 * s, &rl, MUTED, Event::Tab(T_CLOUD_ROUTES));
        y += 34.0 * s;
        let by = y + 4.0 * s;
        let ready = f.shown.as_ref().is_some_and(|(_, pl)| pl.blockers.is_empty()) && !f.running;
        let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, "Preview", AMBER, Event::Tab(T_CLOUD_PREVIEW));
        let w2 = g.button(p.x + 26.0 * s + w1 + 10.0 * s, by, 14.0 * s, if f.running { "Launching\u{2026}" } else { "Launch" }, if ready { OK } else { DIM }, Event::Tab(T_CLOUD_GO));
        g.button(p.x + 26.0 * s + w1 + w2 + 20.0 * s, by, 14.0 * s, "Close", MUTED, Event::Tab(T_CLOUD_CLOSE));
        // what the preview says, or what the launch is doing
        let out: Vec<String> = if f.running || f.done.lock().map(|d| d.is_some()).unwrap_or(false) { f.lines.lock().map(|l| l.clone()).unwrap_or_default() } else { f.text.clone() };
        let ty = by + 40.0 * s;
        let lh = g.line_height(12.0 * s, Face::Mono) * 1.1;
        let fit = (((p.y + p.h - ty - 14.0 * s) / lh) as usize).max(1);
        if out.is_empty() {
            g.text(p.x + 26.0 * s, ty, 13.0 * s, MUTED, "Preview shows exactly what will be applied to this cluster, and what blocks it.", Face::Sans);
        }
        let start = if f.running { out.len().saturating_sub(fit) } else { 0 };
        for (i, l) in out.iter().skip(start).take(fit).enumerate() {
            let col = if l.starts_with("BLOCKED") { ERR } else if l.starts_with('$') || l.starts_with("---") { crate::fx::CYAN } else if l.starts_with("note:") { AMBER } else { TEXT };
            let t = g.fit(12.0 * s, l, Face::Mono, p.w - 52.0 * s);
            g.text(p.x + 26.0 * s, ty + i as f32 * lh, 12.0 * s, col, &t, Face::Mono);
        }
    }
}

/// The Routes page of the card: the routes added (with a way to remove each), then the route being typed.
fn draw_routes_page(g: &mut Gfx, p: R, s: f32, top: f32, f: &CloudForm, tick: u64) {
    let mut y = top;
    let x = p.x + 26.0 * s;
    let inner = p.w - 52.0 * s;
    g.text(x, y, 13.0 * s, MUTED, "Each route sends a domain name to a service of the tenant. Nothing is applied until Launch.", Face::Sans);
    y += 24.0 * s;
    if f.routes.is_empty() {
        g.text(x, y, 13.0 * s, DIM, "No route yet.", Face::Sans);
        y += 22.0 * s;
    }
    for (i, r) in f.routes.iter().take(RM_SHOWN).enumerate() {
        let target = match &r.target {
            Target::Service { name, port } => format!("{name}:{port}"),
            Target::Local { port } => format!("local:{port}"),
        };
        let cert = match &r.cert {
            Some(CertRef::Secret(n)) => format!(", certificate Secret {n}"),
            Some(CertRef::Vault(n)) => format!(", certificate vault {n}"),
            None => String::new(),
        };
        let line = format!("{}  \u{2192}  {target}  {}{cert}", r.host, if r.scheme == Scheme::Https { "https" } else { "http" });
        let rm = 26.0 * s;
        let t = g.fit(13.0 * s, &line, Face::Mono, inner - rm - 8.0 * s);
        g.text(x, y + 2.0 * s, 13.0 * s, TEXT, &t, Face::Mono);
        let rr = R::new(x + inner - rm, y - 1.0 * s, rm, 22.0 * s);
        g.rrect(rr.x, rr.y, rr.w, rr.h, 6.0 * s, if g.hovered(rr) { crate::gfx::rgb(90, 36, 46) } else { crate::gfx::rgb(52, 28, 38) });
        g.text(rr.x + rr.w / 2.0 - 4.0 * s, rr.y + 3.0 * s, 13.0 * s, ERR, "x", Face::Bold);
        g.hit(rr, Event::Tab(T_CLOUD_RM + i), &format!("remove route {}", r.host));
        y += 24.0 * s;
    }
    if f.routes.len() > RM_SHOWN {
        g.text(x, y, 12.0 * s, DIM, &format!("and {} more (remove one above to see them)", f.routes.len() - RM_SHOWN), Face::Sans);
        y += 20.0 * s;
    }
    y += 6.0 * s;
    let lw = 120.0 * s;
    let focus = |i: usize| f.focus == i && !f.running;
    g.text(x, y + 5.0 * s, 13.0 * s, MUTED, "Domain", Face::Sans);
    f.fields[4].render(g, R::new(x + lw, y, inner - lw, 28.0 * s), focus(4), s, super::blink(tick), Face::Sans, T_CLOUD_FIELD + 4);
    y += 34.0 * s;
    g.text(x, y + 5.0 * s, 13.0 * s, MUTED, "Service : port", Face::Sans);
    let pw = 90.0 * s;
    f.fields[5].render(g, R::new(x + lw, y, inner - lw - pw - 8.0 * s, 28.0 * s), focus(5), s, super::blink(tick), Face::Sans, T_CLOUD_FIELD + 5);
    f.fields[6].render(g, R::new(x + inner - pw, y, pw, 28.0 * s), focus(6), s, super::blink(tick), Face::Sans, T_CLOUD_FIELD + 6);
    y += 34.0 * s;
    g.button(x, y + 2.0 * s, 13.0 * s, if f.https { "Scheme: https" } else { "Scheme: http" }, MUTED, Event::Tab(T_CLOUD_SCHEME));
    y += 34.0 * s;
    if f.https {
        g.button(x, y + 2.0 * s, 13.0 * s, if f.cert_vault { "Certificate: vault" } else { "Certificate: file (as a Secret)" }, MUTED, Event::Tab(T_CLOUD_CERTSRC));
        y += 34.0 * s;
        g.text(x, y + 5.0 * s, 13.0 * s, MUTED, if f.cert_vault { "Vault entry" } else { "Secret name" }, Face::Sans);
        f.fields[7].render(g, R::new(x + lw, y, inner - lw, 28.0 * s), focus(7), s, super::blink(tick), Face::Sans, T_CLOUD_FIELD + 7);
        y += 34.0 * s;
    }
    if let Some(e) = &f.route_error {
        let e = g.fit(12.0 * s, e, Face::Sans, inner);
        g.text(x, y, 12.0 * s, ERR, &e, Face::Sans);
    }
    y += 22.0 * s;
    let w2 = g.button(x, y, 14.0 * s, "Add route", AMBER, Event::Tab(T_CLOUD_ADD));
    g.button(x + w2 + 10.0 * s, y, 14.0 * s, "Done", OK, Event::Tab(T_CLOUD_DONE));
}
