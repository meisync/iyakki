//! Settings: the secretproviders page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::account_port::SecretProvidersPort;
use super::*;

impl SettingsApp {
    pub(super) fn secret_provider_form() -> Form {
        Form::new("Add external secret provider")
            .text("Alias (lowercase id)", "", "e.g. corp-vault — used as ext:corp-vault/path#field")
            .choice("Type", &["HashiCorp Vault / OpenBao (KV v2)", "1Password Connect"], 0)
            .text("Address", "", "https://vault.example.com:8200")
            .secret("Access token", "stored in the system vault")
            .text("KV mount (HashiCorp)", "secret", "secret")
            .text("Namespace (HashiCorp, optional)", "", "")
    }

    pub(super) fn save_secret_provider(&mut self, p: &dyn SecretProvidersPort, form: &Form) -> Result<(), String> {
        let (alias, addr, token, mount, ns) = (form.value(0), form.value(2), form.raw(3), form.value(4), form.value(5));
        if !valid_provider_id(&alias) {
            return Err("Alias: lowercase letters, digits, - and _; must start with a letter.".into());
        }
        if token.is_empty() {
            return Err("An access token is required.".into());
        }
        let key_name = format!("{alias}-token");
        let token_ref = format!("vault:{key_name}");
        let cfg = if form.choice_idx(1) == 0 {
            SecretProvider::Hashicorp { addr, mount: if mount.is_empty() { "secret".into() } else { mount }, namespace: if ns.is_empty() { None } else { Some(ns) }, token_ref }
        } else {
            SecretProvider::Onepassword { addr, token_ref }
        };
        // validate the combination before storing the token
        p.check_provider(&alias, &cfg)?;
        p.store_system_secret(&key_name, &token)?;
        p.save_provider(&alias, cfg)?;
        p.audit("settings.secret_provider.save", &alias);
        Ok(())
    }

    pub(super) fn handle_secret_providers(&mut self, k: &Key, p: &dyn SecretProvidersPort) {
        let list = p.secret_providers();
        match k {
            Key::Up => self.move_sel(-1, list.len()),
            Key::Down => self.move_sel(1, list.len()),
            Key::Char('a') if self.admin(p) => self.open_form(FormKind::SecretProvider, Self::secret_provider_form()),
            Key::Char('d') if self.admin(p) => {
                if let Some((a, _)) = list.get(self.sel()) {
                    self.modal = Some(Modal::Confirm(ConfirmKind::SecretProvider(a.clone()), format!("Remove secret provider “{a}”?")));
                }
            }
            Key::Char('t') => {
                if let Some((a, _)) = list.get(self.sel()) {
                    let (job, a) = (p.provider_test_job(a), a.clone());
                    self.start("testing provider", move || JobOut::Msg(job().map(|_| format!("{a}: reachable, token accepted")).map_err(|e| format!("{a}: {e}"))));
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_secret_providers(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn SecretProvidersPort) {
        let list = p.secret_providers();
        let intro = g.fit(14.0 * s, "External vaults. Use their secrets anywhere a reference is accepted:  ext:<alias>/<path>#<field>", Face::Sans, a.w);
        g.text(a.x, a.y, 14.0 * s, MUTED, &intro, Face::Sans);
        let mut y = a.y + 34.0 * s;
        if list.is_empty() {
            let none = g.fit(15.0 * s, "None configured. Press a to connect HashiCorp Vault / OpenBao or 1Password Connect.", Face::Sans, a.w);
            g.text(a.x, y, 15.0 * s, DIM, &none, Face::Sans);
        }
        for (i, (alias, cfg)) in list.iter().enumerate() {
            let (label, addr) = match cfg {
                SecretProvider::Hashicorp { addr, mount, namespace, .. } => ("HashiCorp/OpenBao", format!("{addr}  ·  mount {mount}{}", namespace.as_ref().map(|n| format!("  ·  ns {n}")).unwrap_or_default())),
                SecretProvider::Onepassword { addr, .. } => ("1Password Connect", addr.clone()),
            };
            let on = i == self.sel();
            row(g, &mut self.tip, i, a.x, y, a.w, 58.0 * s, s, on, alias, &addr, &[(label, ACCENT)]);
            y += 66.0 * s;
        }
    }
}
