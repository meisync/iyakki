//! Lane ST-A: the submit arms of its forms (moved out of settings.rs, S2 of SETTINGS-EXTRACTION-PLAN.md; no behaviour change).

use super::*;

impl SettingsApp {
    /// The forms of lane ST-A: NotMine when the kind is another lane's.
    pub(super) fn submit_form_a(&mut self, cx: &AppCx, kind: &FormKind, form: &mut Form) -> FormOutcome {
        match kind {
            FormKind::BundleWizard => FormOutcome::Early(self.wiz_submit(cx, form)),
            FormKind::NewSecret(system) => FormOutcome::Result({
                let (name, value) = (form.value(0), form.raw(1));
                if *system && !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                if !valid_secret_name(&name) {
                    Err("Name: lowercase letters, digits, . _ - (max 64).".into())
                } else if value.is_empty() {
                    Err("Enter the secret value.".into())
                } else {
                    self.store_key(cx, &name, &value, *system).map(|_| format!("Stored `{name}`."))
                }
            }),
            FormKind::Keyfile => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let path = form.value(0);
                let r = Self::svc(cx).enable_keyfile(&path);
                if r.is_ok() {
                    cx.audit.record(&cx.session.user, "vault.keyfile.enable", &path);
                }
                r.map(|_| format!("Key file created at {path}; the system vault will open at boot."))
            }),
            FormKind::SecretProvider => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                self.save_secret_provider(cx, form).map(|_| "Saved secret provider.".to_string())
            }),
            FormKind::Persist(dev, needs_force) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let typed = form.value(0);
                let erase_ok = !*needs_force || form.value(1).trim() == "ERASE";
                if typed.trim().trim_start_matches("/dev/") != dev.as_str() {
                    Err(format!("Type {dev} exactly to confirm."))
                } else if !erase_ok {
                    Err("This disk holds data. Type ERASE (capitals) to replace it.".to_string())
                } else {
                    let (env, dev, force) = (self.storage.clone(), dev.clone(), *needs_force);
                    cx.audit.record(&cx.session.user, "storage.persist", &dev);
                    self.start("preparing the data disk", move || {
                        JobOut::Msg(iyakki_core_api::device_types::storage::persist(&env.roots, env.mounter.as_ref(), &env.mke2fs, &env.scratch, &dev, force, Some(&env.data_dir)).map(|r| format!("{} is now the IYAKKI data disk ({} files copied). Reboot to start using it.", r.device, r.files_copied)))
                    });
                    Ok("Working… this takes a few seconds.".to_string())
                }
            }),
            FormKind::NewVirtualDisk => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let Some(v) = self.storage.virtual_disks.clone() else { return FormOutcome::Early(None) };
                match iyakki_core_api::device_types::storage::virt::parse_size(&form.value(0)) {
                    None => Err("Write a size such as 8G or 512M.".to_string()),
                    Some(bytes) => v.add(bytes).map(|name| {
                        cx.audit.record(&cx.session.user, "storage.virtual.add", &name);
                        self.refresh_devices();
                        format!("Made the virtual disk {name} ({}).", iyakki_core_api::device_types::storage::human(bytes))
                    }),
                }
            }),
            FormKind::RemoveVirtualDisk(name) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let Some(v) = self.storage.virtual_disks.clone() else { return FormOutcome::Early(None) };
                if form.value(0).trim().trim_start_matches("/dev/") != name.as_str() {
                    Err(format!("Type {name} exactly to confirm."))
                } else {
                    v.remove(name).map(|_| {
                        cx.audit.record(&cx.session.user, "storage.virtual.remove", name);
                        self.refresh_devices();
                        format!("Removed {name}.")
                    })
                }
            }),
            FormKind::Matrix => FormOutcome::Result(self.matrix_submit(cx, form)),
            FormKind::Backup(k) => FormOutcome::Result(self.backup_submit(cx, &k, form)),
            FormKind::Mem(f) => FormOutcome::Result(self.mem_submit(*f, form)),
            FormKind::TimeServer => FormOutcome::Result(self.time_submit(cx, form)),
            FormKind::HostName => FormOutcome::Result(self.host_name_submit(cx, form)),
            FormKind::UpdateFolder => FormOutcome::Result(self.upd_folder_submit(form)),
            FormKind::UpdateBundle => FormOutcome::Result(self.upd_bundle_submit(form, cx)),
            FormKind::UpdateFetch => FormOutcome::Result(self.upd_fetch_submit(form, cx)),
            FormKind::UpdateSource => FormOutcome::Result(self.upd_source_submit(cx, form)),
            FormKind::Install(d) => FormOutcome::Result(self.inst_submit(cx, d, form)),
            FormKind::EncHome => FormOutcome::Early(self.ench_submit(cx, form)),
            FormKind::UnlockVault => FormOutcome::Result({
                let user = cx.session.user.clone();
                // through the core contract: `vault.unlock` takes the local seat, which only a person at the machine can make
                let r = {
                    use iyakki_core_api::contract::{CallContext, CallError, ErrorCode, LocalSeat};
                    let ctx = CallContext::local(&user, cx.session.policy.manage_settings);
                    match LocalSeat::claim(&ctx).and_then(|seat| match cx.services.contract_local().vault() { Some(v) => v.unlock(&seat, &form.raw(0)), None => Err(CallError::new(ErrorCode::Denied, "vault.unavailable", "this core does not serve the vault")) }) {
                        Ok(_) => Ok(()),
                        Err(e) if e.code == ErrorCode::NotFound => Err("you have no personal vault yet; it is created when you sign in with your password".to_string()),
                        Err(_) => Err("that password does not open your vault".to_string()),
                    }
                };
                if r.is_ok() {
                    cx.audit.record(&user, "vault.user.unlock", "");
                }
                r.map(|_| "Your vault is open. Passwords that were waiting for it are being saved.".to_string())
            }),
            FormKind::Password => FormOutcome::Result({
                let (old, new, again) = (form.raw(0), form.raw(1), form.raw(2));
                if new != again {
                    Err("The new passwords don't match.".into())
                } else {
                    let user = cx.session.user.clone();
                    let r = Ok::<_, String>(cx.store.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).and_then(|mut s| s.set_password(&cx.session, &user, Some(&old), &new).map_err(|e| match e {
                        AccountError::Forbidden => "The current password is wrong.".to_string(),
                        other => other.to_string(),
                    }));
                    if r.is_ok() {
                        Self::svc(cx).on_password_change(&user, &old, &new);
                        cx.audit.record(&user, "account.password", "");
                    }
                    r.map(|_| "Password changed. Your personal vault keeps working with the new one.".to_string())
                }
            }),
            _ => FormOutcome::NotMine,
        }
    }
}

impl SettingsApp {
    pub(super) fn apply_pick_a(&mut self, cx: &AppCx, kind: PickKind, item: &str) -> bool {
        if matches!(kind, PickKind::Zone | PickKind::Computer(_)) && !self.admin(cx) {
            return true;
        }
        if kind == PickKind::Zone {
            self.time_pick(cx, item);
            return true;
        }
        if let PickKind::Computer(k) = kind {
            self.computer_pick(cx, k, item);
            return true;
        }
        false
    }
}

impl SettingsApp {
    pub(super) fn apply_confirm_a(&mut self, cx: &AppCx, kind: ConfirmKind) -> bool {
        if matches!(kind, ConfirmKind::WizardRun | ConfirmKind::WizardFinish) {
            self.wiz_confirm(cx, kind);
            return true;
        }
        if matches!(kind, ConfirmKind::EncMove | ConfirmKind::EncRemovePlain) {
            self.ench_confirm(cx, kind);
            return true;
        }
        if let ConfirmKind::Computer(act) = kind {
            self.computer_confirm(cx, act);
            return true;
        }
        if kind == ConfirmKind::MemApply {
            if self.admin(cx) && self.mem.writable {
                self.mem_apply(cx);
            }
            return true;
        }
        let r: Result<String, String> = match &kind {
            ConfirmKind::SecretProvider(a) if self.admin(cx) => {
                let r = cx.settings().lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove_secret_provider(a).map(|_| format!("Removed `{a}`.")).map_err(|e| e.to_string());
                Self::svc(cx).refresh_secret_providers();
                r
            }
            ConfirmKind::Secret(system, name) => {
                if *system && !self.admin(cx) {
                    return true;
                }
                let r = if *system { cx.vault().system_delete(name) } else { cx.vault().user_delete(&cx.session.user, name) };
                r.map(|_| format!("Deleted secret `{name}`.")).map_err(|e| e.to_string())
            }
            ConfirmKind::Reboot if self.admin(cx) && cx.session.policy.power => {
                cx.audit.record(&cx.session.user, "power.reboot", "storage");
                cx.host.reboot();
                Ok("Restarting…".to_string())
            }
            ConfirmKind::EncMove | ConfirmKind::EncRemovePlain => Ok(String::new()), // handled above (ench_confirm)
            ConfirmKind::ResetPersonal => cx.vault().reset_user_vault(&cx.session.user).map(|_| "Personal vault removed; a new empty one is created at your next sign-in.".to_string()),
            _ => return false,
        };
        self.confirm_done(cx, r);
        true
    }
}
