//! Settings → Developer mode (administrators): switch it on or off with an optional automatic switch-off, pair a computer (the code and
//! the certificate fingerprint are shown here, with the addresses to type), the paired computers (revoke one), and the activity log of
//! everything they did. The rules and the work are in `iyakki_agent::devmode`; the listener is `iyakki_remote::devserver`.

use crate::safeclamp::SafeClamp;
use super::*;
use iyakki_shared::devmode::scopes::Scope;
use iyakki_agent_api::DevModeApi;
use iyakki_shared::devmode_types as dm;

/// The automatic switch-off choices, in hours (0: stays on until switched off).
pub(super) const HOURS: [u32; 6] = [4, 1, 8, 24, 72, 0];
/// What the "Extend" button adds to the switch-off time.
pub(super) const EXTEND_HOURS: u32 = 4;

/// What the page keeps between frames.
#[derive(Default)]
pub(super) struct DevPanel {
    /// Index into [`HOURS`] for the next switch-on.
    pub(super) hours: usize,
    /// Show the activity log instead of the computers.
    pub(super) log: bool,
    /// Whether it was on when the page was last drawn (for the buttons).
    on: bool,
    /// The scope the switches act on (index into `Scope::ALL`).
    scope: usize,
    /// The web-service family the web API switch acts on (index into the grantable families).
    api: usize,
    /// The core contract scope the switch acts on (index into the grantable contract scopes).
    core: usize,
    /// The "Connect over USB-C" page (key `u`) is showing instead of the computers list.
    pub(super) usb_open: bool,
    pub(super) usb: super::usbc_page::UsbPanel,
    /// A pairing code is on the screen / a computer is approved automatically (for the buttons).
    pairing_shown: bool,
    auto_shown: bool,
    /// How far the page is scrolled (pixels) and the most it can be (set when drawn).
    pub(super) scroll: f32,
    pub(super) max: f32,
    /// The computer whose "Remove this computer" waits for a second press (the question is on the page).
    remove_armed: Option<String>,
    /// The key (`r`, `e`, `Q`, `B`) whose switching OFF waits for a second press: a double tap on a switch must not undo a standing
    /// grant by accident (it used to: the second tap of a quick pair switched it off again, or ended every grant with developer mode).
    off_armed: Option<char>,
    /// The box for the relay's certificate fingerprint (key `F`): typing, Enter keeps it, Esc leaves it.
    pin: iyakki_uikit::ui::TextField,
    pin_focus: bool,
}

/// The id of the relay fingerprint box (see the other field ids of Settings: 931 USB code, 932 roles).
pub(super) const PIN_ID: usize = 933;

fn hours_text(h: u32) -> String {
    if h == 0 { "never (until switched off)".into() } else { format!("after {h} h") }
}

impl SettingsApp {
    fn devmode(cx: &AppCx) -> Arc<dyn DevModeApi> {
        crate::agent().host_probe().devmode(cx.data_dir())
    }

    pub(super) fn devmode_keys(cx: &AppCx) -> Vec<dm::DevKey> {
        Self::devmode(cx).keys()
    }

    /// Events of the relay fingerprint box that must not reach the rest of Settings. True when consumed.
    pub(super) fn relay_pin_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        if self.devel.usb_open || !self.admin(cx) {
            return false;
        }
        match ev {
            Event::FocusAt(id, x) if *id == PIN_ID => {
                self.devel.pin_focus = true;
                self.devel.pin.click(*x);
                true
            }
            Event::Paste(t) if self.devel.pin_focus => {
                self.devel.pin.paste(t);
                true
            }
            // a click on a button below is not typing: it leaves the box and presses that button
            Event::Key(_) if self.devel.pin_focus && crate::ui::from_click() => {
                self.devel.pin_focus = false;
                false
            }
            Event::Key(k) if self.devel.pin_focus => {
                match k {
                    Key::Esc => self.devel.pin_focus = false,
                    Key::Enter => {
                        self.devel.pin_focus = false;
                        let (p, user) = (&cx.session.policy, cx.session.user.clone());
                        let text = self.devel.pin.value.clone();
                        match Self::devmode(cx).set_relay_pin(p, &user, &text) {
                            Ok(()) => {
                                cx.audit.record(&user, "settings.developer", if text.trim().is_empty() { "relay certificate fingerprint removed" } else { "relay certificate fingerprint set" });
                                self.ok(if text.trim().is_empty() { "The relay's certificate fingerprint is removed: an https relay is not used." } else { "The relay's certificate fingerprint is kept. An https relay is only used for that certificate." });
                            }
                            Err(e) => self.fail(e),
                        }
                    }
                    other => {
                        self.devel.pin.handle(other);
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub(super) fn handle_developer(&mut self, k: &Key, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        if self.devel.usb_open {
            self.handle_usb(k, cx);
            return;
        }
        let d = Self::devmode(cx);
        let p = &cx.session.policy;
        let user = cx.session.user.clone();
        if !matches!(k, Key::Char('x') | Key::Delete) {
            self.devel.remove_armed = None;
        }
        if !matches!(k, Key::Char(c) if self.devel.off_armed == Some(*c)) {
            self.devel.off_armed = None;
        }
        match k {
            Key::Char('u') => self.devel.usb_open = true,
            Key::Up => self.move_sel(-1, Self::devmode_keys(cx).len()),
            Key::Down => self.move_sel(1, Self::devmode_keys(cx).len()),
            Key::PageUp => self.dev_wheel(3),
            Key::PageDown => self.dev_wheel(-3),
            // copy the pairing code to type on the other computer
            Key::Char('C') => match d.pairing() {
                Some(pc) if d.is_on() => {
                    crate::ui::clipboard::copy(&pc.code);
                    self.ok("The pairing code is copied.");
                }
                _ => self.fail("No pairing code is showing: press p first.".to_string()),
            },
            Key::Char('e') | Key::Enter if !d.is_on() => match d.enable(p, &user, HOURS[self.devel.hours]) {
                Ok(_) => {
                    cx.audit.record(&user, "devmode.on", &hours_text(HOURS[self.devel.hours]));
                    self.ok(format!("Developer mode is on (port {}). It switches off {}. Pair a computer with p.", d.state().port(), hours_text(HOURS[self.devel.hours])));
                }
                Err(e) => self.fail(e),
            },
            Key::Char('e') if self.devel.off_armed != Some('e') && (!d.restart_grant_list().is_empty() || !d.trusted_list().is_empty()) => {
                self.devel.off_armed = Some('e');
                self.ok("Switching developer mode off ends the restart grants and trusted-computer switches too. Press e again to switch it off; any other key cancels.");
            }
            Key::Char('e') => match d.disable(p, &user) {
                Ok(()) => {
                    cx.audit.record(&user, "devmode.off", "switched off in Settings");
                    self.ok("Developer mode is off: the port is closed and every request is refused.");
                }
                Err(e) => self.fail(e),
            },
            Key::Char('h') => {
                self.devel.hours = (self.devel.hours + 1) % HOURS.len();
                let h = HOURS[self.devel.hours];
                if d.is_on() {
                    // apply to the running session at once
                    match d.enable(p, &user, h) {
                        Ok(_) => self.ok(format!("Switches off {} from now.", hours_text(h))),
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.ok(format!("When switched on it will switch off {}.", hours_text(h)));
                }
            }
            Key::Char('p') => match d.start_pairing(p, &user) {
                Ok(_) => {
                    cx.audit.record(&user, "devmode.pair.open", "pairing code shown");
                    self.ok("Type the code on the computer: iyakki dev pair <address> (good for 5 minutes).");
                }
                Err(e) => self.fail(e),
            },
            // let the firewall through: the pairing (one time) when a code is on the screen, else the computer the hint names
            Key::Char('f') => {
                let hint_src = self.dev_fw_hint(d.as_ref()).and_then(|h| h.split(" from ").nth(1).map(|x| x.trim_end_matches(": Allow").to_string()));
                if d.pairing().is_some() {
                    match d.allow_pairing_through_firewall(p, &user) {
                        Ok(_) => {
                            cx.audit.record(&user, "devmode.firewall.request", "pairing (one time)");
                            self.ok("The firewall lets the local network reach the developer port for ten minutes, or until a computer has paired. The pairing code and the fingerprint are still what protects it.");
                        }
                        Err(e) => self.fail(e),
                    }
                } else if let Some(src) = hint_src {
                    match d.allow_peer(p, &user, &src) {
                        Ok(()) => {
                            cx.audit.record(&user, "devmode.firewall.request", &format!("{src} while developer mode is on"));
                            self.ok(format!("The firewall lets {src} reach the developer port while developer mode is on."));
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.fail("Nothing to allow: press p to show a pairing code first, then f.".to_string());
                }
            }
            Key::Char('n') => {
                let now_on = !d.network_wide();
                match d.set_network_wide(p, &user, now_on) {
                    Ok(()) => self.ok(if now_on { "The firewall now lets the whole network of each paired computer reach the developer port (only paired keys can act)." } else { "The firewall lets only the address of each paired computer in." }),
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('c') => {
                d.cancel_pairing();
                self.ok("The pairing code is cancelled.");
            }
            // "Remove this computer": the first press asks (the page says what goes), the second one removes
            Key::Char('x') | Key::Delete => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                if let Some(k) = rows.get(sel) {
                    if self.devel.remove_armed.as_deref() != Some(k.name.as_str()) {
                        self.devel.remove_armed = Some(k.name.clone());
                        self.ok(format!("Press x again (or tap Confirm) to remove {}; any other key cancels.", k.name));
                        return;
                    }
                    self.devel.remove_armed = None;
                    match d.remove_pairing(p, &user, &k.name) {
                        Ok(said) => {
                            cx.audit.record(&user, "devmode.revoke", &k.name);
                            self.ok(said);
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
            Key::Char('l') => self.devel.log = !self.devel.log,
            // push the automatic switch-off 4 h further (the timer of "h" starts it again from now)
            Key::Char('t') => match d.extend(p, &user, EXTEND_HOURS) {
                Ok(st) => {
                    cx.audit.record(&user, "devmode.extend", &format!("{EXTEND_HOURS} h"));
                    self.ok(format!("Developer mode now switches off at {}.", dm::stamp(st.until)));
                }
                Err(e) => self.fail(e),
            },
            // every diagnostic scope for the selected computer, in one go: look, files, screen, cluster, shell (still approved here)
            Key::Char('G') => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                if let Some(row) = rows.get(sel) {
                    match d.grant_diagnostic(p, &user, &row.name) {
                        Ok(nk) => {
                            cx.audit.record(&user, "devmode.scope.grant", &format!("{} diagnostic scopes", nk.name));
                            self.ok(format!("{} may now help diagnose: {}. A shell still needs your approval each time; updates and power stay off.", nk.name, nk.scope_text()));
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.fail("No computer is paired yet.".to_string());
                }
            }
            Key::Char('s') => self.devel.scope = (self.devel.scope + 1) % Scope::ALL.len(),
            // the web service's api:* scopes of the selected computer: i chooses the family, m cycles nothing -> reading only -> all
            Key::Char('i') => self.devel.api = (self.devel.api + 1) % api_families().len(),
            Key::Char('m') => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                let Some(row) = rows.get(sel) else {
                    self.fail("Pair a computer first.".to_string());
                    return;
                };
                let fam = api_families()[self.devel.api % api_families().len()];
                let held = d.api_scopes(&row.name);
                let next = match api_state(&held, fam) {
                    0 => Some(format!("{fam}:read")),
                    1 => Some(fam.to_string()),
                    _ => None,
                };
                let r = match &next {
                    Some(sc) => d.set_api_scope(p, &user, &row.name, sc, true),
                    None => d.set_api_scope(p, &user, &row.name, fam, false),
                };
                match r {
                    Ok(now) => {
                        cx.audit.record(&user, "devmode.api.set", &format!("{} {}", row.name, next.as_deref().unwrap_or(fam)));
                        self.ok(format!("{} may now call in the web service: {}.", row.name, if now.is_empty() { "nothing".to_string() } else { now.join(", ") }));
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char(' ') | Key::Char('g') => {
                let sc = Scope::ALL[self.devel.scope % Scope::ALL.len()];
                if matches!(k, Key::Char('g')) {
                    // the computer that is about to pair
                    let on = !d.pairing().is_some_and(|p| p.scopes.iter().any(|x| x == sc.id()));
                    match d.set_pairing_scope(p, &user, sc, on) {
                        Ok(l) => self.ok(format!("The computer that pairs will get: {}.", l.join(", "))),
                        Err(e) => self.fail(e),
                    }
                } else {
                    let rows = Self::devmode_keys(cx);
                    let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                    if let Some(row) = rows.get(sel) {
                        let on = !row.has(sc);
                        match d.set_scope(p, &user, &row.name, sc, on) {
                            Ok(nk) => {
                                cx.audit.record(&user, if on { "devmode.scope.grant" } else { "devmode.scope.revoke" }, &format!("{} {}", nk.name, sc.id()));
                                self.ok(format!("{} now holds: {}.", nk.name, nk.scope_text()));
                            }
                            Err(e) => self.fail(e),
                        }
                    }
                }
            }
            Key::Char('a') | Key::Char('4') | Key::Char('8') | Key::Char('1') | Key::Char('o') | Key::Char('d') | Key::Char('y') => {
                let Some(req) = d.shell_pending().into_iter().next() else {
                    self.fail("No computer is asking for shell access right now.".to_string());
                    return;
                };
                let r = match k {
                    Key::Char('a') => d.shell_approve(p, &user, &req.computer, 10).map(|g| format!("{} may run commands for 10 minutes{}. The top bar shows it.", g.computer, if g.as_root { " as root" } else { "" })),
                    Key::Char(c @ ('4' | '8' | '1')) => {
                        let hours = match c { '4' => 4, '8' => 8, _ => 12 };
                        d.shell_approve(p, &user, &req.computer, hours * 60).map(|g| format!("{} may run commands for {hours} hours{}. The top bar shows it.", g.computer, if g.as_root { " as root" } else { "" }))
                    }
                    Key::Char('o') => d.shell_approve(p, &user, &req.computer, 0).map(|g| format!("{} may run commands for one session.", g.computer)),
                    // approve this one and stop asking for a while: a plain shell only (never as root, power or patching)
                    Key::Char('y') if req.as_root => Err("A request to run as root is always asked each time: press a or o.".to_string()),
                    Key::Char('y') => d.shell_approve(p, &user, &req.computer, 10).and_then(|g| d.shell_auto_on(p, &user, &req.computer, iyakki_shared::devmode::grants::AUTO_DEFAULT_HOURS).map(|_| format!("{} may run commands; its next requests are approved by themselves for {} hours (never as root). w stops it.", g.computer, iyakki_shared::devmode::grants::AUTO_DEFAULT_HOURS))),
                    _ => d.shell_deny(p, &user, &req.computer).map(|_| format!("{} was refused.", req.computer)),
                };
                match r {
                    Ok(m) => {
                        cx.audit.record(&user, "devmode.shell", &m);
                        self.ok(m)
                    }
                    Err(e) => self.fail(e),
                }
            }
            // the box "Allow restarts from this computer without asking" for the selected computer: lasts as long as the developer
            // connection (no time of its own), restart and switch-off only
            Key::Char('r') => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                let Some(row) = rows.get(sel) else {
                    self.fail("Pair a computer first.".to_string());
                    return;
                };
                if d.restart_grant_active(&row.name) {
                    if self.devel.off_armed != Some('r') {
                        self.devel.off_armed = Some('r');
                        self.ok(format!("{} may restart this device without asking. Press r (or tap) again to stop that; any other key keeps it.", row.name));
                        return;
                    }
                    self.devel.off_armed = None;
                    match d.restart_grant_off(p, &user, &row.name) {
                        Ok(()) => {
                            cx.audit.record(&user, "devmode.restart.grant_off", &row.name);
                            self.ok(format!("{} must ask for approval to restart this device again.", row.name));
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    match d.restart_grant_on(p, &user, &row.name) {
                        Ok(v) => {
                            cx.audit.record(&user, "devmode.restart.grant_on", &row.name);
                            self.ok(format!("{} may restart or switch off this device without asking until {}. Nothing else.", row.name, if v.until > 0 { dm::stamp(v.until) } else { "developer mode is switched off".to_string() }));
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
            // "Keep until I switch it off" under the restart box: the grant, and developer mode with it, stay until switched off by hand
            Key::Char('b') => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                let Some(row) = rows.get(sel) else {
                    self.fail("Pair a computer first.".to_string());
                    return;
                };
                let kept = d.restart_grant_list().into_iter().any(|v| v.computer == row.name && v.persist);
                if kept {
                    match d.restart_grant_on(p, &user, &row.name) {
                        Ok(v) => {
                            cx.audit.record(&user, "devmode.restart.grant_on", &row.name);
                            self.ok(format!("{} is no longer kept: it may restart this device without asking until {}.", row.name, dm::stamp(v.until)));
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    match d.restart_grant_persist(p, &user, &row.name) {
                        Ok(_) => {
                            cx.audit.record(&user, "devmode.restart.grant_persist", &row.name);
                            self.ok(format!("Restarts allowed until you switch it off: {} may restart or switch off this device without asking, and developer mode stays on until you switch it off. Nothing else.", row.name));
                        }
                        Err(e) => self.fail(e),
                    }
                }
            }
            // "Trusted dev computer" (Q kept, J 4 h, Z 12 h) and "Patches without asking" (B): for dev and test machines. Switching ON is the
            // person's tap on this page (a hard ask); switching OFF needs a second press so a double tap cannot undo it by accident.
            Key::Char(c @ ('Q' | 'J' | 'Z' | 'B')) => {
                use dm::trusted::{TrustFor, TrustLevel};
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                let Some(row) = rows.get(sel) else {
                    self.fail("Pair a computer first.".to_string());
                    return;
                };
                let level = if *c == 'B' { TrustLevel::Patches } else { TrustLevel::Everything };
                let held = d.trusted_list().into_iter().find(|t| t.computer == row.name);
                let same_kept = held.as_ref().is_some_and(|t| t.level == level && t.kept()) && matches!(c, 'Q' | 'B');
                if same_kept {
                    if self.devel.off_armed != Some(*c) {
                        self.devel.off_armed = Some(*c);
                        self.ok(format!("{} is {}. Press {c} (or tap) again to switch that off; any other key keeps it.", row.name, if level == TrustLevel::Patches { "allowed to patch without asking" } else { "a trusted dev computer" }));
                        return;
                    }
                    self.devel.off_armed = None;
                    match d.trusted_off(p, &user, &row.name) {
                        Ok(()) => {
                            cx.audit.record(&user, "devmode.trusted.off", &row.name);
                            self.ok(format!("{} must ask for approval again.", row.name));
                        }
                        Err(e) => self.fail(e),
                    }
                    return;
                }
                let how = match c {
                    'J' => TrustFor::Hours(4),
                    'Z' => TrustFor::Hours(12),
                    _ => TrustFor::Kept,
                };
                match d.trusted_on(p, &user, &row.name, level, how) {
                    Ok(v) => {
                        cx.audit.record(&user, "devmode.trusted.on", &row.name);
                        self.ok(format!("{} {} {}. Every use is logged; the top bar shows it. {}", row.name, if level == TrustLevel::Patches { "may patch this device without asking" } else { "is a trusted dev computer: its approvals are answered yes by themselves" }, if v.kept() { "until you switch it off (developer mode stays on until then)".to_string() } else { format!("until {}", dm::stamp(v.until)) }, if level == TrustLevel::Patches { "Nothing else." } else { "Passwords, the vault and root are never covered." }));
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('w') => {
                let trusted = d.shell_auto_list();
                if trusted.is_empty() {
                    self.fail("No computer is approved automatically right now.".to_string());
                    return;
                }
                let mut m = String::new();
                for a in &trusted {
                    if let Err(e) = d.shell_auto_off(p, &user, &a.computer) {
                        self.fail(e);
                        return;
                    }
                    m = format!("{}{}{}", m, if m.is_empty() { "" } else { ", " }, a.computer);
                }
                cx.audit.record(&user, "devmode.shell.auto_off", &m);
                self.ok(format!("{m} must ask for approval again."));
            }
            Key::Char('W') => {
                // the experimental WASM tile switch (off by default; the chat tool wasm_tiles_set does the same)
                let on = !cx.storeapi().wasm_tiles_enabled();
                let r = cx.settings().lock().map_err(|_| "the settings are busy".to_string()).and_then(|mut st| st.update(|x| x.wasm_tiles = on).map_err(|e| e.to_string()));
                match r {
                    Ok(_) => {
                        cx.audit.record(&user, "settings.wasm_tiles", if on { "on" } else { "off" });
                        self.ok(if on { "WASM tiles (experimental) are on: tile apps with a .wasm module run in the sandboxed tile host." } else { "WASM tiles are off." });
                    }
                    Err(e) => self.fail(e),
                }
            }
            // the typed core contract over the developer channel (off by default; the chat tools core_contract_switch and
            // core_contract_scope_set do the same): K switches the channel, j chooses a contract scope, z grants or takes it back
            Key::Char('K') => {
                let on = !d.core_enabled();
                match d.set_core_enabled(p, &user, on) {
                    Ok(()) => {
                        cx.audit.record(&user, "settings.developer", &format!("core contract {}", if on { "on" } else { "off" }));
                        self.ok(if on { "The core contract channel is on. A computer holds no contract scope until you grant one below." } else { "The core contract channel is off." });
                    }
                    Err(e) => self.fail(e),
                }
            }
            // connections to this machine through an owner-run relay (off by default, and only with an address set; the chat tools relay_status,
            // relay_switch and relay_approve_computer do the same): Y switches it
            Key::Char('Y') => {
                let on = !d.relay_enabled();
                if on && d.relay_url().is_empty() {
                    self.fail("Give the relay's address first (ask the assistant: relay_switch with an address), then switch it on.".to_string());
                    return;
                }
                match d.set_relay_enabled(p, &user, on) {
                    Ok(()) => {
                        cx.audit.record(&user, "settings.developer", &format!("relay {}", if on { "on" } else { "off" }));
                        self.ok(if on { "Connections through the relay are on. A computer's first contact waits for your yes on this machine." } else { "Connections through the relay are off." });
                    }
                    Err(e) => self.fail(e),
                }
            }
            // the fingerprint of the relay's certificate (the chat tool relay_switch with certificate_fingerprint does the same): F opens the box
            Key::Char('F') => {
                self.devel.pin = iyakki_uikit::ui::TextField::new("SHA-256 fingerprint of the relay's certificate (64 hex digits)", false);
                self.devel.pin.max = 200;
                self.devel.pin.set(&d.relay_pin());
                self.devel.pin_focus = true;
                self.ok("Type or paste the fingerprint of the relay's certificate, then Enter. An empty box takes it away. Esc cancels.");
            }
            // the socket of a UI program outside the process (off by default; the chat tools ui_socket_status and ui_socket_set do the
            // same; the scopes and the account stay as they are): U switches it, through the core handle
            Key::Char('U') => {
                let on = !cx.core.ui_socket().enabled();
                match cx.core.ui_socket().set_enabled(on) {
                    Ok(_) => {
                        cx.audit.record(&user, "settings.developer", &format!("ui socket {}", if on { "on" } else { "off" }));
                        self.ok(if on { "The UI socket is on: a UI program on this machine can use it with the scopes and account set by chat (ui_socket_set)." } else { "The UI socket is off." });
                    }
                    Err(e) => self.fail(e),
                }
            }
            // put the person back at the desktop after the shell restarted, without the password (developer and test machines only; the
            // chat tools session_resume_status and session_resume_set do the same): R switches it. It takes effect only while developer
            // mode is on, and the vaults stay locked.
            Key::Char('R') => {
                let on = !cx.core.session_resume().switch_on();
                match cx.core.session_resume().set_switch(on) {
                    Ok(()) => {
                        cx.audit.record(&user, "settings.developer", &format!("resume after restart {}", if on { "on" } else { "off" }));
                        self.ok(if on { "Resume after a restart is on: while developer mode is on, a restart of the desktop puts the person back without the password (the vault stays locked)." } else { "Resume after a restart is off." });
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('j') => self.devel.core = (self.devel.core + 1) % core_scopes().len(),
            Key::Char('z') => {
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                let Some(row) = rows.get(sel) else {
                    self.fail("Pair a computer first.".to_string());
                    return;
                };
                let sc = core_scopes()[self.devel.core % core_scopes().len()];
                let on = !d.core_scopes(&row.name).iter().any(|x| x == sc);
                match d.set_core_scope(p, &user, &row.name, sc, on) {
                    Ok(now) => {
                        cx.audit.record(&user, "settings.developer", &format!("core scope {sc} {} for {}", if on { "granted" } else { "taken back" }, row.name));
                        self.ok(format!("{} may now use in the core contract: {}.", row.name, if now.is_empty() { "nothing".to_string() } else { now.join(", ") }));
                    }
                    Err(e) => self.fail(e),
                }
            }
            Key::Char('k') => {
                // end the selected computer's shell access and disconnect all its sessions
                let rows = Self::devmode_keys(cx);
                let sel = self.sel[self.tab].min(rows.len().saturating_sub(1));
                if let Some(row) = rows.get(sel) {
                    let _ = d.shell_end(p, &user, &row.name);
                    let n = Self::devmode(cx).end_sessions_of(&row.name);
                    cx.audit.record(&user, "devmode.disconnect", &row.name);
                    self.ok(format!("{} is disconnected ({n} session(s) ended, shell access ended).", row.name));
                }
            }
            _ => {}
        }
    }

    /// A click or tap on a line of a list: put the cursor on `row` (a scope) and press the key the line stands for.
    pub(super) fn dev_sub(&mut self, row: u8, k: &Key, cx: &AppCx) {
        if self.devel.usb_open {
            return;
        }
        // lines 100 and up are the web API families (key m cycles the family under the finger)
        if row >= 120 {
            self.devel.core = (row - 120) as usize;
        } else if row >= 100 {
            self.devel.api = (row - 100) as usize;
        } else {
            self.devel.scope = row as usize % Scope::ALL.len();
        }
        self.handle_developer(k, cx);
    }

    /// The mouse wheel (positive = up): scrolls the page, or the list of checks on the USB-C page.
    pub(super) fn dev_wheel(&mut self, d: i32) {
        if self.devel.usb_open {
            self.devel.usb.top = (self.devel.usb.top as i32 - d).clamp(0, 6) as usize;
        } else {
            self.devel.scroll = (self.devel.scroll - d as f32 * crate::scrolleng::SCREEN_NOTCH_PX * self.nav_s.max(0.5)).clamp_safe(0.0, self.devel.max);
        }
    }

    /// "the firewall blocks the developer port ...: Allow" while developer mode is on, the firewall is in force and nothing lets it in.
    pub(super) fn dev_fw_hint(&self, d: &dyn DevModeApi) -> Option<String> {
        let blocked = iyakki_fw::service::block_log().recent();
        iyakki_fw::devrules::hint(&self.fw.handle.lock(), &blocked, d.state().port(), d.is_on())
    }

    pub(super) fn developer_buttons(&self) -> Vec<Btn> {
        if self.devel.usb_open {
            return self.usb_buttons();
        }
        let on = self.devel.on;
        let mut v: Vec<Btn> = vec![(if on { "Switch off" } else { "Switch on" }, if on { ERR } else { ACCENT }, 'e'), ("Switch-off time", TEXT, 'h')];
        if on {
            v.push(("Pair a computer", ACCENT, 'p'));
            v.push(("Allow in the firewall", ACCENT, 'f'));
            v.push(("Whole network on/off", TEXT, 'n'));
            v.push(("Cancel code", TEXT, 'c'));
        }
        if on {
            v.push(("Extend 4 h", TEXT, 't'));
        }
        if self.devel.pairing_shown {
            v.push(("Copy code", ACCENT, 'C'));
        }
        if self.devel.auto_shown {
            v.push(("Ask each time again", TEXT, 'w'));
        }
        v.push(("Core contract on/off", TEXT, 'K'));
        v.push(("Relay on/off", TEXT, 'Y'));
        v.push(("Relay certificate", TEXT, 'F'));
        v.push(("UI socket on/off", TEXT, 'U'));
        v.push(("Resume after restart on/off", TEXT, 'R'));
        v.push(("Contract scope on/off", TEXT, 'z'));
        v.push(("Grant all diagnostic scopes", ACCENT, 'G'));
        v.push(("Next scope", TEXT, 's'));
        v.push(("Scope on/off", TEXT, ' '));
        if on {
            v.push(("Approve shell", ERR, 'a'));
            v.push(("Approve 4 h", ERR, '4'));
            v.push(("Approve 8 h", ERR, '8'));
            v.push(("Approve 12 h", ERR, '1'));
            v.push(("Shell once", ERR, 'o'));
            v.push(("Deny shell", TEXT, 'd'));
        }
        if on {
            v.push(("Restarts without asking on/off", TEXT, 'r'));
            v.push(("Patches without asking on/off", TEXT, 'B'));
            v.push(("Trusted dev computer on/off", ERR, 'Q'));
            v.push(("Trusted 4 h", TEXT, 'J'));
            v.push(("Trusted 12 h", TEXT, 'Z'));
        }
        v.push(("Disconnect", ERR, 'k'));
        if self.devel.remove_armed.is_some() {
            v.push(("Confirm: remove this computer", ERR, 'x'));
        } else {
            v.push(("Remove this computer", ERR, 'x'));
        }
        v.push(("Activity log", TEXT, 'l'));
        v.push(("Connect over USB-C", ACCENT, 'u'));
        v
    }

    pub(super) fn render_developer(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        if self.devel.usb_open {
            self.render_usb(g, a, s, cx);
            return;
        }
        // the page is taller than the window when a code, hints and a request show: it scrolls (wheel, finger, Page keys) and
        // everything in it that can be clicked is cut to the window, so nothing under the buttons is hit
        self.devel.scroll = self.devel.scroll.clamp_safe(0.0, self.devel.max);
        let top = a.y - self.devel.scroll;
        let shifted = R::new(a.x, top, a.w, a.h + self.devel.scroll);
        let end = g.clipped(a, |g| self.render_developer_body(g, shifted, s, cx));
        self.devel.max = (end - top + 12.0 * s - a.h).max(0.0);
    }

    /// The whole page from `a.y` down; returns where it ends.
    fn render_developer_body(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) -> f32 {
        let d = Self::devmode(cx);
        let on = d.is_on();
        self.devel.on = on;
        self.devel.pairing_shown = on && d.pairing().is_some();
        self.devel.auto_shown = on && !d.shell_auto_list().is_empty();
        let mut y = a.y + controls::section_title(g, a.x, a.y, s, "Install and run apps from your own computer");
        for l in [
            "Like adb for IYAKKI: a paired computer can install, upgrade and remove tile apps, skills, MCP servers and extensions, read their logs, open them on this screen and take a screenshot (iyakki dev ...).".to_string(),
            "What it installs is labelled Developer (local), goes through the STORE's checks and is never updated from a store. Driver packs and kernel modules cannot be installed this way. Off by default; every action is logged.".to_string(),
        ] {
            for w in g.wrap(13.5 * s, &l, Face::Sans, a.w) {
                g.text(a.x, y, 13.5 * s, MUTED, &w, Face::Sans);
                y += 19.0 * s;
            }
        }
        y += 6.0 * s;
        let st = d.state();
        let (word, col) = if on { (d.status_text(), rgb(255, 196, 94)) } else { ("Off: nothing listens, every request is refused".to_string(), OK) };
        let head = g.fit(16.0 * s, &format!("Developer mode: {word}"), Face::Bold, a.w);
        g.text(a.x, y, 16.0 * s, col, &head, Face::Bold);
        y += 26.0 * s;
        // the switch-off time: tap the line to go to the next choice (h)
        tap_line(g, a, &mut y, s, 13.0, MUTED, Face::Sans, &format!("Port {} (TLS) · next switch-on switches off {} · tap to change (h)", st.port(), hours_text(HOURS[self.devel.hours])), Some(Event::Key(Key::Char('h'))));
        y += 6.0 * s;
        if let Some(p) = d.pairing().filter(|_| on) {
            y += controls::section_title(g, a.x, y, s, "Pair a computer (tap the code to copy it)");
            g.text(a.x, y, 30.0 * s, ACCENT, &p.code, Face::Mono);
            g.hit(R::new(a.x, y - 2.0 * s, a.w, 44.0 * s), Event::Key(Key::Char('C')), "Copy the pairing code");
            y += 46.0 * s;
            let fp = Self::devmode(cx).cert_fingerprint().unwrap_or_else(|| "(the certificate is being made: look again in a moment)".into());
            let half = fp.len().div_ceil(2).min(48);
            for part in [&fp[..half.min(fp.len())], &fp[half.min(fp.len())..]] {
                if !part.is_empty() {
                    g.text(a.x, y, 13.0 * s, TEXT, part.trim_start_matches(':'), Face::Mono);
                    y += 18.0 * s;
                }
            }
            let addrs: Vec<String> = cx.os.net().ipv4_addresses().unwrap_or_default().into_iter().filter(|(_, ip)| !ip.starts_with("127.")).map(|(_, ip)| format!("{ip}:{}", st.port())).collect();
            let line = format!("On the computer: iyakki dev pair {} — then type this code. The fingerprint above must match what it prints. The computer will act as {} ({} left).", addrs.first().cloned().unwrap_or_else(|| format!("<this device's address>:{}", st.port())), p.account, {
                let left = (p.expires - d.now()).max(0);
                format!("{}:{:02}", left / 60, left % 60)
            });
            for w in g.wrap(13.0 * s, &line, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
                y += 18.0 * s;
            }
            g.text(a.x, y, 13.0 * s, rgb(255, 196, 94), "What the new computer will be allowed (tap a line, or s then g):", Face::Sans);
            y += 22.0 * s;
            for (i, sc) in Scope::ALL.into_iter().enumerate() {
                let held = p.scopes.iter().any(|x| x == sc.id());
                let cursor = if i == self.devel.scope % Scope::ALL.len() { "▶" } else { " " };
                let col = if held && sc.dangerous() { ERR } else if held { rgb(255, 196, 94) } else { DIM };
                let t = g.fit(13.0 * s, &format!("{cursor} [{}] {:<16} {}", if held { "x" } else { " " }, sc.id(), sc.describe()), Face::Mono, a.w);
                g.text(a.x, y + 5.0 * s, 13.0 * s, col, &t, Face::Mono);
                g.hit(R::new(a.x, y, a.w, SCOPE_PITCH * s), Event::Sub(i as u8, Key::Char('g')), &format!("New computer: {}", sc.id()));
                y += SCOPE_PITCH * s;
            }
            y += 8.0 * s;
        }
        if on {
            let w = d.fw_want();
            if let Some(h) = self.dev_fw_hint(d.as_ref()) {
                tap_line(g, a, &mut y, s, 13.0, rgb(255, 196, 94), Face::Sans, &format!("{h} (tap or f)"), Some(Event::Key(Key::Char('f'))));
            }
            if w.pair_until > 0 {
                tap_line(g, a, &mut y, s, 13.0, TEXT, Face::Sans, &format!("Firewall: the local network may reach the developer port until {} or until a computer has paired (rule: developer mode pairing (one time)).", dm::stamp(w.pair_until)), None);
            }
            let (t, ev) = if w.network_wide {
                ("Firewall: paired computers are allowed from their whole network (/24). Trade-off: any computer on this network can reach the developer port, though only paired keys can act. Tap or n turns it off.".to_string(), Event::Key(Key::Char('n')))
            } else {
                ("Firewall: only the address of each paired computer is let in (default). If its address changes, tap the hint above or press f, or tap here (n) to allow its whole network (any computer there could then reach the port; only paired keys can act).".to_string(), Event::Key(Key::Char('n')))
            };
            tap_line(g, a, &mut y, s, 13.0, MUTED, Face::Sans, &t, Some(ev));
            y += 6.0 * s;
            for t in d.shell_auto_list() {
                let line = format!("{} runs commands without asking until {} (never as root). Tap to ask each time again (w)", t.computer, dm::stamp(t.until));
                tap_line(g, a, &mut y, s, 13.0, MUTED, Face::Sans, &line, Some(Event::Key(Key::Char('w'))));
            }
            for (n, r) in d.shell_pending().into_iter().enumerate() {
                let line = format!("{} asks to RUN COMMANDS on this device as {}{}{}.{}", r.computer, r.account, if r.as_root { " (as root)" } else { "" }, if r.why.is_empty() { String::new() } else { format!(" — {}", r.why) }, if n == 0 { "" } else { " (waits its turn)" });
                for w in g.wrap(14.0 * s, &line, Face::Bold, a.w) {
                    g.text(a.x, y, 14.0 * s, ERR, &w, Face::Bold);
                    y += 20.0 * s;
                }
                if n == 0 {
                    // the answers are buttons as well as keys; they act on the first request in the line
                    let mut choices: Vec<(&str, [u8; 4], char)> = vec![("10 minutes", ERR, 'a'), ("4 hours", ERR, '4'), ("8 hours", ERR, '8'), ("12 hours", ERR, '1'), ("One session", ERR, 'o')];
                    if !r.as_root {
                        choices.push(("Yes, and stop asking for 8 h", ERR, 'y'));
                    }
                    choices.push(("Refuse", TEXT, 'd'));
                    y = request_buttons(g, a, y, s, &choices);
                }
                y += 6.0 * s;
            }
            let sessions = Self::devmode(cx).sessions();
            if !sessions.is_empty() {
                y += controls::section_title(g, a.x, y, s, "Open sessions (Disconnect ends the selected computer's)");
                let now = d.now();
                let ends = if st.until > 0 { format!("closes at the latest when developer mode switches off {}", dm::stamp(st.until)) } else { "stays until closed or developer mode is switched off".to_string() };
                for x in sessions.iter().take(8) {
                    let t = g.fit(13.0 * s, &format!("{} ({}) · {} · open {} (since {}) · {ends}", x.computer, x.account, x.what, dm::pairings::ago(now, x.started).trim_end_matches(" ago"), dm::stamp(x.started)), Face::Mono, a.w);
                    g.text(a.x, y, 13.0 * s, TEXT, &t, Face::Mono);
                    y += 18.0 * s;
                }
                y += 6.0 * s;
            }
        }
        if self.devel.log {
            y += controls::section_title(g, a.x, y, s, "Activity log (newest first)");
            let acts = d.activity(60);
            if acts.is_empty() {
                g.text(a.x, y, 14.0 * s, DIM, "Nothing yet.", Face::Sans);
                y += 22.0 * s;
            }
            for x in acts {
                let t = g.fit(12.5 * s, &x.line(), Face::Mono, a.w);
                g.text(a.x, y, 12.5 * s, if x.ok { TEXT } else { ERR }, &t, Face::Mono);
                y += 18.0 * s;
            }
            return y;
        }
        let wasm_on = cx.storeapi().wasm_tiles_enabled();
        tap_line(g, a, &mut y, s, 13.0, if wasm_on { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &format!("WASM tiles (experimental): {} · tap here or press W to turn {}", if wasm_on { "on" } else { "off (default)" }, if wasm_on { "off" } else { "on" }), Some(Event::Key(Key::Char('W'))));
        let core_on = d.core_enabled();
        tap_line(g, a, &mut y, s, 13.0, if core_on { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &format!("Core contract channel: {} · tap here or press K to turn {}. A computer holds no contract scope until you grant one below.", if core_on { "on" } else { "off (default)" }, if core_on { "off" } else { "on" }), Some(Event::Key(Key::Char('K'))));
        let relay_on = d.relay_enabled();
        tap_line(g, a, &mut y, s, 13.0, if relay_on { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &format!("Relay: {} · tap here or press Y to turn {}. {}", if relay_on { "on" } else { "off (default)" }, if relay_on { "off" } else { "on" }, d.relay_summary()), Some(Event::Key(Key::Char('Y'))));
        let pin = d.relay_pin();
        tap_line(g, a, &mut y, s, 13.0, MUTED, Face::Sans, &format!("Relay certificate fingerprint: {} · tap here or press F to type it. An https relay is only used for the certificate you name here.", if pin.is_empty() { "none (an https relay is not used)".to_string() } else { iyakki_proxy::certinfo::fingerprint_groups(&pin) }), Some(Event::Key(Key::Char('F'))));
        if self.devel.pin_focus {
            let r = R::new(a.x, y, a.w.min(640.0 * s), 30.0 * s);
            self.devel.pin.render(g, r, true, s, (self.ticks / 4) % 2 == 0, Face::Mono, PIN_ID);
            y += 38.0 * s;
        }
        let ui_on = cx.core.ui_socket().enabled();
        tap_line(g, a, &mut y, s, 13.0, if ui_on { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &format!("UI socket (a UI program on this machine): {} · tap here or press U to turn {}. A unix socket, no network port.", if ui_on { "on" } else { "off (default)" }, if ui_on { "off" } else { "on" }), Some(Event::Key(Key::Char('U'))));
        let rs_on = cx.core.session_resume().switch_on();
        tap_line(g, a, &mut y, s, 13.0, if rs_on { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &format!("Resume after a restart (developer machines): {} · tap here or press R to turn {}. While developer mode is on, a restarted desktop puts the person back without the password; the vault stays locked.", if rs_on { "on" } else { "off (default)" }, if rs_on { "off" } else { "on" }), Some(Event::Key(Key::Char('R'))));
        y += controls::section_title(g, a.x, y, s, "Paired computers (tap one to choose it)");
        let rows = Self::devmode_keys(cx);
        if rows.is_empty() {
            let (t, k) = if on { ("None yet. Tap here or press p to show a pairing code.", 'p') } else { ("None. Tap here to switch developer mode on (e), then pair a computer (p).", 'e') };
            tap_line(g, a, &mut y, s, 14.0, DIM, Face::Sans, t, Some(Event::Key(Key::Char(k))));
            return y;
        }
        let sel = self.sel[self.tab].min(rows.len() - 1);
        for (i, k) in rows.iter().enumerate() {
            let used = if k.last_used == 0 { "never used".to_string() } else { format!("last used {}", dm::stamp(k.last_used)) };
            row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, i == sel, &k.name, &format!("key {} · acts as {} · paired {} ({}) by {} · {used}{} · scopes: {}", k.short(), k.account, dm::stamp(k.added), dm::pairings::ago(d.now(), k.added), k.added_by, if k.last_used == 0 { String::new() } else { format!(" ({})", dm::pairings::ago(d.now(), k.last_used)) }, k.scope_text()), &[(if on { "can connect" } else { "mode off" }, if on { rgb(255, 196, 94) } else { DIM })]);
            y += 66.0 * s;
        }
        // the switches of the selected computer: one line per scope; a tap on a line turns it on or off (space does the same)
        if let Some(k) = rows.get(sel) {
            if let Some(pairing) = d.paired_computer(&k.name) {
                let armed = self.devel.remove_armed.as_deref() == Some(k.name.as_str());
                let text = if armed { format!("{} Tap here or press x to confirm; any other key cancels.", pairing.removal_text()) } else { format!("Remove this computer (x): {}", pairing.removal_text()) };
                tap_line(g, a, &mut y, s, 13.0, if armed { ERR } else { MUTED }, Face::Sans, &text, Some(Event::Key(Key::Char('x'))));
                y += 6.0 * s;
            }
            y += controls::section_title(g, a.x, y, s, &format!("What {} may do (tap a line to turn it on or off; s: next, space: on/off)", k.name));
            let note = "Grant all diagnostic scopes gives: debug, files, input, kube and shell (a shell still needs your approval each time). It never gives updates, restart/power, port forwarding, system files, password fields or settings.";
            for w in g.wrap(12.5 * s, note, Face::Sans, a.w) {
                g.text(a.x, y, 12.5 * s, MUTED, &w, Face::Sans);
                y += 17.0 * s;
            }
            for (i, sc) in Scope::ALL.into_iter().enumerate() {
                let held = k.has(sc);
                let cursor = if i == self.devel.scope % Scope::ALL.len() { "▶" } else { " " };
                let col = if held && sc.dangerous() { ERR } else if held { rgb(255, 196, 94) } else { DIM };
                let t = g.fit(13.0 * s, &format!("{cursor} [{}] {:<16} {}", if held { "x" } else { " " }, sc.id(), sc.describe()), Face::Mono, a.w);
                g.text(a.x, y + 5.0 * s, 13.0 * s, col, &t, Face::Mono);
                g.hit(R::new(a.x, y, a.w, SCOPE_PITCH * s), Event::Sub(i as u8, Key::Char(' ')), &format!("{}: {}", k.name, sc.id()));
                y += SCOPE_PITCH * s;
            }
            // the web service's api:* scopes: a program of this computer that signs in with its key may call only these families
            y += 6.0 * s;
            y += controls::section_title(g, a.x, y, s, &format!("What programs of {} may call in the web service (tap a line: none, reading only, all; i: next, m: change)", k.name));
            let held = d.api_scopes(&k.name);
            for (i, fam) in api_families().into_iter().enumerate() {
                let st = api_state(&held, fam);
                let cursor = if i == self.devel.api % api_families().len() { "▶" } else { " " };
                let col = if st == 2 { rgb(255, 196, 94) } else if st == 1 { rgb(200, 210, 120) } else { DIM };
                let t = g.fit(13.0 * s, &format!("{cursor} [{}] {:<14} {}", ["  ", "r ", "x "][st as usize], fam, iyakki_shared::devmode::apiscopes::FAMILIES.iter().find(|(f, _)| *f == fam).map_or("", |(_, w)| *w)), Face::Mono, a.w);
                g.text(a.x, y + 5.0 * s, 13.0 * s, col, &t, Face::Mono);
                g.hit(R::new(a.x, y, a.w, SCOPE_PITCH * s), Event::Sub(100 + i as u8, Key::Char('m')), &format!("{}: {fam}", k.name));
                y += SCOPE_PITCH * s;
            }
            // the typed core contract: what a client of this computer may do over the contract channel (the local marker never)
            y += 6.0 * s;
            y += controls::section_title(g, a.x, y, s, &format!("What {} may do in the core contract (tap a line to turn it on or off; j: next, z: on/off)", k.name));
            let held = d.core_scopes(&k.name);
            for (i, sc) in core_scopes().into_iter().enumerate() {
                let on_sc = held.iter().any(|x| x == sc);
                let cursor = if i == self.devel.core % core_scopes().len() { "▶" } else { " " };
                let t = g.fit(13.0 * s, &format!("{cursor} [{}] {:<16}", if on_sc { "x" } else { " " }, sc), Face::Mono, a.w);
                g.text(a.x, y + 5.0 * s, 13.0 * s, if on_sc { rgb(255, 196, 94) } else { DIM }, &t, Face::Mono);
                g.hit(R::new(a.x, y, a.w, SCOPE_PITCH * s), Event::Sub(120 + i as u8, Key::Char('z')), &format!("{}: {sc}", k.name));
                y += SCOPE_PITCH * s;
            }
            // the standing restart grant: no time of its own, it ends with the developer connection
            if on && k.has(Scope::Power) {
                y += 6.0 * s;
                let granted = d.restart_grant_list().into_iter().find(|v| v.computer == k.name);
                let ends = |u: i64| if u > 0 { format!("until {}", dm::stamp(u)) } else { "until developer mode is switched off".to_string() };
                let line = match &granted {
                    Some(v) if v.persist => "[x] Allow restarts from this computer without asking (restart and switch-off only, every use is logged). Restarts allowed until you switch it off. Tap to stop (r)".to_string(),
                    Some(v) => format!("[x] Allow restarts from this computer without asking ({}, the same time as this developer connection; restart and switch-off only, every use is logged{}). Tap to stop (r)", ends(v.until), if v.awaiting_start { "; waiting for the device to report a successful start" } else { "" }),
                    None => "[ ] Allow restarts from this computer without asking (lasts as long as this developer connection; restart and switch-off only, every use is logged). Tap to allow (r)".to_string(),
                };
                tap_line(g, a, &mut y, s, 13.0, if granted.is_some() { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &line, Some(Event::Key(Key::Char('r'))));
                // the second key: keep it until switched off by hand (developer mode stays on with it)
                let kept = granted.as_ref().is_some_and(|v| v.persist);
                let line2 = if kept {
                    "[x] Keep until I switch it off: developer mode will stay on until you switch it off, and this computer may restart the device any time. Tap to go back to the normal time limit (b)"
                } else {
                    "[ ] Keep until I switch it off (also keeps developer mode on: its automatic switch-off no longer applies; you end it by unticking, disconnecting the computer or switching developer mode off). Tap to keep (b)"
                };
                tap_line(g, a, &mut y, s, 13.0, if kept { rgb(255, 196, 94) } else { MUTED }, Face::Sans, line2, Some(Event::Key(Key::Char('b'))));
            }
            // Patches without asking and the trusted dev computer: for dev and test machines (any computer: each approval still needs the
            // scope it opens)
            if on {
                use dm::trusted::TrustLevel;
                y += 6.0 * s;
                let held = d.trusted_list().into_iter().find(|t| t.computer == k.name);
                let ends = |u: i64| if u > 0 { format!("until {}", dm::stamp(u)) } else { "until you switch it off".to_string() };
                let patch_on = held.as_ref().is_some_and(|t| t.level == TrustLevel::Patches);
                let all_on = held.as_ref().is_some_and(|t| t.level == TrustLevel::Everything);
                let l1 = match &held {
                    Some(t) if patch_on => format!("[x] Patches without asking ({}): this computer may patch the apps program, tile apps and layers (iyakki dev patch, push-apps) with no card; the kernel, boot menu and firmware keep their own approval. Tap to stop (B)", ends(t.until)),
                    _ => "[ ] Patches without asking: this computer may patch the apps program, tile apps and layers (iyakki dev patch, push-apps) with no card; kept until you switch it off. Tap to allow (B)".to_string(),
                };
                tap_line(g, a, &mut y, s, 13.0, if patch_on { rgb(255, 196, 94) } else { MUTED }, Face::Sans, &l1, Some(Event::Key(Key::Char('B'))));
                let l2 = match &held {
                    Some(t) if all_on => format!("[x] TRUSTED DEV COMPUTER ({}): for dev and test machines only. Every approval this computer would raise (commands, patching, restart) is answered yes by itself and logged; passwords, the vault and root are never covered. Tap to switch off (Q)", ends(t.until)),
                    _ => "[ ] Trusted dev computer: for dev and test machines only. Every approval this computer would raise (commands, patching, restart) is answered yes by itself and logged; passwords, the vault and root are never covered. Kept until you switch it off (developer mode stays on); J: 4 h, Z: 12 h. Tap to allow (Q)".to_string(),
                };
                tap_line(g, a, &mut y, s, 13.0, if all_on { ERR } else { MUTED }, Face::Sans, &l2, Some(Event::Key(Key::Char('Q'))));
            }
        }
        y
    }
}

/// The scopes of the core contract an administrator can grant (`local` never).
fn core_scopes() -> Vec<&'static str> {
    iyakki_shared::devmode::contract::GRANTABLE.to_vec()
}

/// The families of the web service an administrator can grant (`api:admin` never).
fn api_families() -> Vec<&'static str> {
    iyakki_shared::devmode::apiscopes::FAMILIES.iter().map(|(f, _)| *f).filter(|f| *f != "api:admin").collect()
}

/// 0 nothing, 1 reading only, 2 everything of the family.
fn api_state(held: &[String], fam: &str) -> u8 {
    if held.iter().any(|s| s == fam) {
        2
    } else if held.iter().any(|s| s.strip_suffix(":read") == Some(fam)) {
        1
    } else {
        0
    }
}

/// Height of a tappable scope line (a finger needs about 44 px; the line is drawn small, the target is this tall).
const SCOPE_PITCH: f32 = 40.0;

/// A wrapped paragraph; with `ev` the whole paragraph (at least 36 px tall) is a target for a click or tap.
#[allow(clippy::too_many_arguments)]
fn tap_line(g: &mut Gfx, a: R, y: &mut f32, s: f32, size: f32, col: [u8; 4], face: Face, text: &str, ev: Option<Event>) {
    let top = *y;
    for w in g.wrap(size * s, text, face, a.w) {
        g.text(a.x, *y, size * s, col, &w, face);
        *y += (size + 5.0) * s;
    }
    if let Some(ev) = ev {
        let h = (*y - top).max(36.0 * s);
        g.hit(R::new(a.x, top - 2.0 * s, a.w, h), ev, text);
        *y = top + h;
    }
}

/// A row of buttons (44 px tall targets) wrapping at the page width; returns the y below them.
fn request_buttons(g: &mut Gfx, a: R, y: f32, s: f32, choices: &[(&str, [u8; 4], char)]) -> f32 {
    let (mut bx, mut by) = (a.x, y);
    for (label, col, key) in choices {
        let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
        if bx + bw > a.x + a.w && bx > a.x {
            bx = a.x;
            by += BTN_PITCH * s;
        }
        bx += g.button_touch(bx, by + (BTN_PITCH - 14.0 * 2.1) * s / 2.0, 14.0 * s, label, *col, Event::Key(Key::Char(*key)), BTN_PITCH * s) + 8.0 * s;
    }
    by + BTN_PITCH * s
}
