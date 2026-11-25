//! Settings: the vault page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::account_port::VaultPort;
use super::*;

impl SettingsApp {
    /// The secret names of the vault this page shows: the system vault for an administrator on its tab, else the person's own.
    pub(super) fn vault_page_names(&self, p: &dyn VaultPort) -> Result<Vec<String>, VaultError> {
        p.vault_names(self.section == 0 && p.is_admin())
    }

    pub(super) fn handle_vault(&mut self, k: &Key, p: &dyn VaultPort) {
        let admin = p.is_admin();
        let names = self.vault_page_names(p).unwrap_or_default();
        let system = self.section == 0 && admin;
        match k {
            Key::Tab if admin => {
                self.section = 1 - self.section;
                self.sel[self.tab] = 0;
            }
            Key::Up => self.move_sel(-1, names.len()),
            Key::Down => self.move_sel(1, names.len()),
            Key::Char('n') => {
                let title = if system { "New system secret" } else { "New personal secret" };
                let f = Form::new(title).text("Name", "", "lowercase, e.g. github-token").secret("Value", "never shown again");
                self.open_form(FormKind::NewSecret(system), f);
            }
            Key::Char('d') => {
                if let Some(n) = names.get(self.sel()) {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Secret(system, n.clone()), format!("Delete secret “{n}”? This cannot be undone.")));
                }
            }
            Key::Char('l') if system => {
                p.lock_system_vault();
                p.audit("vault.system.lock", "");
                self.ok("System vault locked. An admin with access unlocks it by signing in.");
            }
            Key::Char('f') if system => {
                let f = Form::new("Unattended unlock key file").path("Path for the new key file", &p.data_dir().join("vault.key").display().to_string(), "absolute path", crate::filepicker::PathKind::Save, &[]);
                self.open_form(FormKind::Keyfile, f);
            }
            Key::Char('u') if !system && !p.user_vault_unlocked() => {
                let f = Form::new("Unlock my vault").secret("Your login password", "opens only your own vault; you stay signed in");
                self.open_form(FormKind::UnlockVault, f);
            }
            Key::Char('R') if !system => self.modal = Some(Modal::Confirm(ConfirmKind::ResetPersonal, "Delete your personal vault and all secrets in it?".into())),
            _ => {}
        }
    }

    pub(super) fn render_vault(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn VaultPort) {
        let admin = p.is_admin();
        let system = self.section == 0 && admin;
        let (exists, unlocked) = p.system_vault_state();
        let mut y = a.y;
        let pill = |g: &mut Gfx, x: f32, y: f32, label: &str, on: bool| {
            let w = g.text_width(14.0 * s, label, Face::Bold) + 28.0 * s;
            let hot = g.hovered(R::new(x, y, w, 30.0 * s));
            if on {
                g.rrect_glow(x, y, w, 30.0 * s, 15.0 * s, 4.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 80]);
                g.rrect(x, y, w, 30.0 * s, 15.0 * s, ACCENT);
            } else {
                crate::fx::row_at(g, x, y, w, 30.0 * s, 15.0 * s, false, hot, ACCENT);
            }
            if !on {
                g.hit(R::new(x, y, w, 30.0 * s), Event::Key(Key::Tab), label); // switches system <-> personal
            }
            g.text(x + 14.0 * s, y + 6.0 * s, 14.0 * s, if on { rgb(10, 14, 28) } else { MUTED }, label, Face::Bold);
            w + 8.0 * s
        };
        let mut px = a.x;
        if admin {
            px += pill(g, px, y, "System vault", system);
        }
        pill(g, px, y, "My vault", !system);
        y += 44.0 * s;
        if system {
            let (st, good) = vaultstate_page::status_words(true, exists, unlocked);
            g.text(a.x, y, 15.0 * s, if good { OK } else { ERR }, &format!("Status: {st}"), Face::Bold);
            y += 24.0 * s;
            let slots = p.system_slots().join(", ");
            let slots = g.fit(13.0 * s, &format!("Key holders: {}", if slots.is_empty() { "-".into() } else { slots }), Face::Sans, a.w);
            g.text(a.x, y, 13.0 * s, MUTED, &slots, Face::Sans);
            y += 30.0 * s;
        } else {
            // the lock state comes through the core contract (`vault.status`)
            let un = p.user_vault_unlocked();
            let (st, _) = vaultstate_page::status_words(false, true, un);
            g.text(a.x, y, 15.0 * s, if un { OK } else { ERR }, &format!("Status: {st}"), Face::Bold);
            y += 30.0 * s;
            if let Some(w) = p.vault_waiting_line(un) {
                let w = g.fit(13.0 * s, &w, Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, MUTED, &w, Face::Sans);
                y += 28.0 * s;
            }
        }
        match self.vault_page_names(p) {
            Err(e) => {
                g.text(a.x, y, 15.0 * s, ERR, &e.to_string(), Face::Sans);
            }
            Ok(names) => {
                if names.is_empty() {
                    for w in g.wrap(15.0 * s, "No secrets yet. Press n to add one. Values are never displayed after saving.", Face::Sans, a.w) {
                        g.text(a.x, y, 15.0 * s, DIM, &w, Face::Sans);
                        y += 21.0 * s;
                    }
                }
                for (i, n) in names.iter().enumerate() {
                    if y > a.y + a.h - 60.0 * s {
                        break;
                    }
                    let on = i == self.sel();
                    row(g, &mut self.tip, i, a.x, y, a.w, 48.0 * s, s, on, n, &format!("reference:  {}:{n}", if system { "vault" } else { "uvault" }), &[("••••••••", DIM)]);
                    y += 56.0 * s;
                }
            }
        }
    }
}
