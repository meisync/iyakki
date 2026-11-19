//! Settings -> Fleet (experimental): the pair groups of this computer, their members and the numbers they share, one-time join codes,
//! requests to join, the work sent and received, and the accept-work switch (F-FLEET-09; `iyakki_agent::fleet*`,
//! docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md section 5).
//!
//! The page and the assistant's `fleet_*` tools are one thing: every change here calls the same code as the tool (`FleetApi`) after a card that
//! says what will change, answered with y; the core asks as well (`fleet.join`, `fleet.manage`, `fleet.assign`, `fleet.receive`: hard asks).
//! A join code is shown here, to the person at this screen, and nowhere else.
//!
//! Keys: Up/Down choose a row; c create a fleet; o offer a join code; j join with a code; y or Enter approve a request, accept an offer or
//! take a task; n refuse, drop or decline; x remove the chosen member (or leave, on this computer's own row); l leave; t trust; A
//! administrator; m the accept-work setting; T the TLS switch of the channel; a where the chosen member is reached (an address, a pinned
//! tls:// address or a relay:// mailbox); u the relay switch; i apply the work handed back to the integration branch (and send the follow-ups);
//! f put the integration branch into your own branch; r look again.

use super::*;

pub(super) use iyakki_shared::fleetview::FleetAct;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Member { me: bool, admin: bool, trusted: bool },
    Request,
    Pending { offered: bool },
    Inbox { waiting: bool },
}

#[derive(Clone, Debug)]
pub(super) struct FRow {
    pub(super) kind: Kind,
    pub(super) id: String,
    pub(super) fleet: String,
    pub(super) title: String,
    pub(super) sub: String,
    pub(super) chip: (&'static str, [u8; 4]),
}

pub(super) struct Snapshot {
    pub(super) head: Vec<String>,
    pub(super) rows: Vec<FRow>,
    pub(super) mode: String,
    pub(super) fleets: usize,
    pub(super) tls: bool,
    pub(super) relay: String,
}

impl SettingsApp {
    pub(super) fn fleet_snapshot(&self, cx: &AppCx) -> Result<Snapshot, String> {
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let st = cx.services.fleet_view(cx.os.clone(), cx.os.fleet(), Arc::new(move |a, d| audit.record(&user, a, d)))?;
        let mut head = vec![format!("This computer: fingerprint {}; {}.", st.my_fingerprint, st.listening.map(|p| format!("listening on port {p}")).unwrap_or_else(|| "not listening (it starts when a fleet is made or joined)".into()))];
        head.push(format!("Work sent by other members: {}.", st.accept.label()));
        head.push(format!("Channel: {} (T changes it). {}", if st.tls { "TLS, the certificate pinned by the other computers" } else { "plain, for the local network" }, if st.relay.is_empty() { "Relay: off (u switches it on, for a member that cannot be reached directly).".to_string() } else { format!("Relay: on, this computer is reached at {} (u switches it off).", st.relay) }));
        for o in &st.offers {
            head.push(format!("Join code {} for {}: tell it to the other computer, with this computer's address and port. {} s left. It works once.", o.code, o.short, o.expires_in));
        }
        let mut rows = vec![];
        for f in &st.fleets {
            for m in &f.members {
                let mut sub = format!("{} · {} · {}", f.name, m.fingerprint, if m.admin { "administrator" } else { "member" });
                match m.transport.as_str() {
                    "tls" => sub.push_str(" · over TLS"),
                    "relay" => sub.push_str(" · through the relay"),
                    "tls-left" => sub.push_str(" · it left TLS: set its new address (a)"),
                    _ => {}
                }
                if let Some(h) = &m.heartbeat {
                    sub = format!("{sub} · {}", h.line());
                } else if !m.me {
                    sub = format!("{sub} · no numbers yet");
                }
                let chip: (&str, [u8; 4]) = match m.state.as_str() {
                    "this computer" | "up" => (if m.me { "this computer" } else { "up" }, OK),
                    "lost" | "refused" => (if m.state == "lost" { "lost" } else { "refused" }, crate::fx::AMBER),
                    _ => ("not heard yet", DIM),
                };
                rows.push(FRow { kind: Kind::Member { me: m.me, admin: m.admin, trusted: m.trusted }, id: m.id.clone(), fleet: f.id.clone(), title: format!("{}{}", m.name, if m.trusted { " (trusted)" } else { "" }), sub, chip });
            }
        }
        for r in &st.requests {
            rows.push(FRow { kind: Kind::Request, id: r.id.clone(), fleet: r.fleet.clone(), title: format!("{} asks to join {}", r.name, r.short), sub: format!("fingerprint {}: approve only if the other computer shows the same", r.fingerprint), chip: ("asks", crate::fx::AMBER) });
        }
        for p in &st.pending {
            let offered = p.offer.is_some();
            rows.push(FRow { kind: Kind::Pending { offered }, id: p.ticket.clone(), fleet: String::new(), title: format!("Our request to {}", p.addr), sub: p.offer.clone().unwrap_or_else(|| p.state.clone()), chip: if offered { ("offer", crate::fx::AMBER) } else { ("waiting", DIM) } });
        }
        for x in st.inbox.iter().rev().take(20) {
            rows.push(FRow { kind: Kind::Inbox { waiting: x.state == "waiting" }, id: x.id.clone(), fleet: x.fleet.clone(), title: format!("Work from {}: {}", x.from_name, x.body.title), sub: format!("{} · no secrets travel; accepting does not start it yet", x.state), chip: if x.state == "waiting" { ("waiting", crate::fx::AMBER) } else { ("answered", OK) } });
        }
        for x in st.outbox.iter().rev().take(10) {
            rows.push(FRow { kind: Kind::Inbox { waiting: false }, id: x.id.clone(), fleet: x.fleet.clone(), title: format!("Sent to {}: {}", x.to_name, x.title), sub: x.state.clone(), chip: ("sent", DIM) });
        }
        Ok(Snapshot { head, rows, mode: st.accept.label().into(), fleets: st.fleets.len(), tls: st.tls, relay: st.relay.clone() })
    }

    pub(super) fn fleet_len(&self, cx: &AppCx) -> usize {
        self.fleet_snapshot(cx).map(|s| s.rows.len()).unwrap_or(0)
    }

    fn fleet_ask(&mut self, act: FleetAct, text: String) {
        self.modal = Some(Modal::Confirm(ConfirmKind::Fleet(act), text));
    }

    pub(super) fn handle_fleet(&mut self, k: &Key, cx: &AppCx) {
        if !self.admin(cx) {
            return;
        }
        let snap = match self.fleet_snapshot(cx) {
            Ok(s) => s,
            Err(e) => return self.fail(e),
        };
        let len = snap.rows.len();
        let sel = self.sel[self.tab].min(len.saturating_sub(1));
        let cur = snap.rows.get(sel).cloned();
        let sole = if snap.fleets == 1 { snap.rows.iter().find(|r| matches!(r.kind, Kind::Member { .. })).map(|r| r.fleet.clone()) } else { None };
        let fleet_of_row = cur.as_ref().map(|r| r.fleet.clone()).filter(|f| !f.is_empty()).or(sole);
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('r') => self.ok("Looked again."),
            Key::Char('c') => self.open_form(FormKind::FleetCreate, Form::new("Create a fleet").text("Fleet name", "", "for example: workshop").text("Name of this computer", "", "how the other members see it")),
            Key::Char('j') => self.open_form(FormKind::FleetJoin, Form::new("Join a fleet").text("Address and port", "", "like 192.168.1.20:7671, from the computer that offers the code").text("Join code", "", "nine digits, works once").text("Name of this computer", "", "how the other members see it")),
            Key::Char('o') => match fleet_of_row {
                Some(f) => self.fleet_ask(FleetAct::Offer(f), "Make a one-time join code (ten minutes) for this fleet? Another computer that knows the code can ask to join; you still approve it.".into()),
                None => self.fail("This computer is in no fleet yet: press c to create one."),
            },
            Key::Char('m') => {
                let next = match snap.mode.as_str() {
                    m if m.starts_with("ask") => "never",
                    m if m.starts_with("accept no") => "members",
                    _ => "ask",
                };
                self.fleet_ask(FleetAct::Mode(next.into()), format!("Work sent by other members: change to \"{}\"?", cx.services.fleet_accept_label(next)));
            }
            Key::Char('T') => self.fleet_ask(FleetAct::Tls(!snap.tls), if snap.tls { "Make the fleet channel plain again (local network only)? Members that pinned its certificate must be given a plain address.".into() } else { "Make the fleet channel use TLS with this computer's certificate? Another computer far away reaches it at tls://ADDRESS:PORT#FINGERPRINT after its person compared the fingerprint.".into() }),
            Key::Char('u') => self.fleet_ask(FleetAct::Relay(snap.relay.is_empty()), if snap.relay.is_empty() { "Switch the relay on? A member that cannot be reached directly can then be reached through a mailbox. Messages stay signed and sealed with the group key; the relay sees only who writes to which mailbox and when.".into() } else { "Switch the relay off? Members given a relay address are not reached until a direct address is set.".into() }),
            Key::Char('a') => match cur {
                Some(FRow { kind: Kind::Member { me: false, .. }, fleet, id, title, .. }) => self.open_form(FormKind::FleetAddress(fleet, id), Form::new(&format!("Where is {title}?")).text("Address", "", "host:port, tls://host:port#fingerprint, or relay://mailbox")),
                _ => self.fail("Choose another member."),
            },
            Key::Char('i') => match fleet_of_row {
                Some(f) => self.fleet_ask(FleetAct::Integrate(f, true), "Apply the work the other members handed back to an integration branch of the open project, run its tests, and send the follow-ups for what did not apply? Your own branch is not touched until you press f.".into()),
                None => self.fail("This computer is in no fleet."),
            },
            Key::Char('f') => match fleet_of_row {
                Some(f) => self.fleet_ask(FleetAct::Finalize(f), "Put the integration branch into your own branch of the open project?".into()),
                None => self.fail("This computer is in no fleet."),
            },
            Key::Char('l') => match fleet_of_row {
                Some(f) => self.fleet_ask(FleetAct::Leave(f), "Leave this fleet? The other members are told and this computer forgets the fleet.".into()),
                None => self.fail("This computer is in no fleet."),
            },
            Key::Char('y') | Key::Enter => match cur {
                Some(FRow { kind: Kind::Request, id, title, sub, .. }) => self.fleet_ask(FleetAct::Approve(id), format!("{title}. {sub}. Approve?")),
                Some(FRow { kind: Kind::Pending { offered: true }, id, sub, .. }) => self.fleet_ask(FleetAct::Accept(id), format!("{sub} Join?")),
                Some(FRow { kind: Kind::Inbox { waiting: true }, id, title, .. }) => self.fleet_ask(FleetAct::Decide(id, true), format!("{title}. Accept it? Running it as a task on this computer is not built yet; it stays in the list as accepted.")),
                _ => {}
            },
            Key::Char('n') => match cur {
                Some(FRow { kind: Kind::Request, id, title, .. }) => self.fleet_ask(FleetAct::Refuse(id), format!("{title}. Refuse?")),
                Some(FRow { kind: Kind::Pending { .. }, id, .. }) => self.fleet_ask(FleetAct::Drop(id), "Drop this request to join?".into()),
                Some(FRow { kind: Kind::Inbox { waiting: true }, id, title, .. }) => self.fleet_ask(FleetAct::Decide(id, false), format!("{title}. Decline it?")),
                _ => {}
            },
            Key::Char('x') => match cur {
                Some(FRow { kind: Kind::Member { me: true, .. }, fleet, .. }) => self.fleet_ask(FleetAct::Leave(fleet), "Leave this fleet? The other members are told and this computer forgets the fleet.".into()),
                Some(FRow { kind: Kind::Member { .. }, fleet, id, title, .. }) => self.fleet_ask(FleetAct::Remove(fleet, id), format!("Remove {title}? The group key changes, so it can no longer read or send fleet messages.")),
                _ => self.fail("Choose a member to remove."),
            },
            Key::Char('t') => match cur {
                Some(FRow { kind: Kind::Member { me: false, trusted, .. }, fleet, id, title, .. }) => self.fleet_ask(FleetAct::Trust(fleet, id, !trusted), format!("{} work from {title} without asking (when the setting is \"members\")?", if trusted { "Stop accepting" } else { "Accept" })),
                _ => self.fail("Choose another member to trust."),
            },
            Key::Char('A') => match cur {
                Some(FRow { kind: Kind::Member { admin, .. }, fleet, id, title, .. }) => self.fleet_ask(FleetAct::Admin(fleet, id, !admin), format!("Make {title} {} an administrator?", if admin { "no longer" } else { "" })),
                _ => self.fail("Choose a member."),
            },
            _ => {}
        }
    }

    pub(super) fn fleet_form_submit(&mut self, cx: &AppCx, kind: &FormKind, form: &Form) -> Result<(), String> {
        let _ = cx;
        let v = |i: usize| form.value(i).trim().to_string();
        match kind {
            FormKind::FleetAddress(fleet, member) => {
                if v(0).is_empty() {
                    return Err("Give the address.".into());
                }
                self.fleet_ask(FleetAct::Address(fleet.clone(), member.clone(), v(0)), format!("Reach that member at {}? The address is kept as given; a pinned tls:// address needs the fingerprint the member shows.", v(0).split('#').next().unwrap_or("")));
            }
            FormKind::FleetCreate => {
                if v(0).is_empty() || v(1).is_empty() {
                    return Err("Give the fleet and this computer a name.".into());
                }
                self.fleet_ask(FleetAct::Create(v(0), v(1)), format!("Create the fleet {}? This computer becomes its administrator and starts listening for other members.", v(0)));
            }
            _ => {
                if v(0).is_empty() || v(1).is_empty() || v(2).is_empty() {
                    return Err("Give the address, the code and a name for this computer.".into());
                }
                self.fleet_ask(FleetAct::Join(v(0), v(1), v(2)), format!("Ask {} to let this computer join its fleet? An administrator there must approve, then you accept.", v(0)));
            }
        }
        Ok(())
    }

    pub(super) fn fleet_confirm(&mut self, cx: &AppCx, act: FleetAct) {
        if !self.admin(cx) {
            return;
        }
        let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
        let r = cx.services.fleet_act(cx.os.clone(), cx.os.fleet(), Arc::new(move |a, d| audit.record(&user, a, d)), act);
        match r {
            Ok(m) => self.ok(m),
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn fleet_buttons(&self) -> Vec<Btn> {
        let (acc, mu) = (ACCENT, TEXT);
        vec![("Create fleet", acc, 'c'), ("Join code", acc, 'o'), ("Join", acc, 'j'), ("Yes / accept", mu, 'y'), ("No / refuse", mu, 'n'), ("Remove", mu, 'x'), ("Trust", mu, 't'), ("Administrator", mu, 'A'), ("Accept work", mu, 'm'), ("TLS", mu, 'T'), ("Address", mu, 'a'), ("Relay", mu, 'u'), ("Integrate", mu, 'i'), ("Finalize", mu, 'f'), ("Leave", mu, 'l')]
    }

    pub(super) fn render_fleet(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mut y = a.y;
        let intro = "Experimental. A fleet is a pair group of IYAKKI computers: they share their disk, load and memory every few seconds and can send each other work. Every change asks you first, and the other computer's person is asked too. Nothing travels that looks like a secret.";
        for w in g.wrap(13.0 * s, intro, Face::Sans, a.w) {
            g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
            y += 18.0 * s;
        }
        y += 8.0 * s;
        let snap = match self.fleet_snapshot(cx) {
            Ok(s) => s,
            Err(e) => {
                for w in g.wrap(14.0 * s, &e, Face::Sans, a.w) {
                    g.text(a.x, y, 14.0 * s, crate::fx::AMBER, &w, Face::Sans);
                    y += 20.0 * s;
                }
                return;
            }
        };
        for l in &snap.head {
            for w in g.wrap(13.0 * s, l, Face::Sans, a.w) {
                g.text(a.x, y, 13.0 * s, TEXT, &w, Face::Sans);
                y += 18.0 * s;
            }
        }
        y += 8.0 * s;
        if snap.rows.is_empty() {
            g.text(a.x, y, 14.0 * s, DIM, "No fleet yet. Press c to create one, or j to join one.", Face::Sans);
            return;
        }
        let sel = self.sel[self.tab].min(snap.rows.len().saturating_sub(1));
        let rh = 64.0 * s;
        for (i, r) in snap.rows.iter().enumerate().take(80) {
            let sub: String = r.sub.chars().take(220).collect();
            row(g, &mut self.tip, i, a.x, y, a.w, rh - 6.0 * s, s, i == sel, &r.title, &sub, &[r.chip]);
            y += rh;
        }
    }
}
