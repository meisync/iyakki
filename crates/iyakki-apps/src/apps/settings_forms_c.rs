//! Lane ST-C: the submit arms of its forms (moved out of settings.rs, S2 of SETTINGS-EXTRACTION-PLAN.md; no behaviour change).

use super::*;

impl SettingsApp {
    /// The forms of lane ST-C: NotMine when the kind is another lane's.
    pub(super) fn submit_form_c(&mut self, cx: &AppCx, kind: &FormKind, form: &mut Form) -> FormOutcome {
        match kind {
            FormKind::FleetCreate | FormKind::FleetJoin | FormKind::FleetAddress(..) => {
                return FormOutcome::Early(match self.fleet_form_submit(cx, &kind, form) {
                    Ok(()) => None,
                    Err(e) => {
                        form.error = Some(e);
                        Some(Modal::Form(kind.clone(), form.clone()))
                    }
                });
            }
            FormKind::WifiJoin(ssid, security) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                match iyakki_core_api::device_types::devctl::wifi::Known::new(ssid, &form.raw(0), *security, false, iyakki_clock::now_unix()) {
                    Ok(k) => {
                        self.join_wifi(cx, k);
                        Ok("Working… this takes a few seconds.".to_string())
                    }
                    Err(e) => Err(e),
                }
            }),
            FormKind::WifiEnterprise(ssid) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let opt = |s: String| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) };
                let method = match form.choice_idx(0) {
                    1 => iyakki_core_api::device_types::devctl::wifi_ext::EapMethod::Ttls,
                    2 => iyakki_core_api::device_types::devctl::wifi_ext::EapMethod::Tls,
                    _ => iyakki_core_api::device_types::devctl::wifi_ext::EapMethod::Peap,
                };
                let e = iyakki_core_api::device_types::devctl::wifi_ext::Enterprise {
                    method,
                    identity: form.value(1),
                    password: opt(form.raw(2)),
                    anonymous_identity: opt(form.value(3)),
                    inner: None,
                    ca_cert: form.value(4),
                    server_name_match: opt(form.value(5)),
                    client_cert: opt(form.value(6)),
                    private_key: opt(form.value(7)),
                    private_key_password: opt(form.raw(8)),
                };
                match iyakki_core_api::device_types::devctl::wifi::Known::new_enterprise(ssid, false, e, iyakki_clock::now_unix()) {
                    Ok(k) => {
                        self.join_wifi(cx, k);
                        Ok("Working… this takes a few seconds.".to_string())
                    }
                    Err(e) => Err(e),
                }
            }),
            FormKind::WifiWep(ssid) => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                if form.choice_idx(1) != 1 {
                    Err(iyakki_core_api::device_types::devctl::wifi_ext::wep_warning().to_string())
                } else {
                match iyakki_core_api::device_types::devctl::wifi::Known::new_wep(ssid, &form.raw(0), false, iyakki_clock::now_unix()) {
                    Ok(k) => {
                        self.join_wifi(cx, k);
                        Ok("Working… this takes a few seconds.".to_string())
                    }
                    Err(e) => Err(e),
                }
                }
            }),
            FormKind::WifiHidden => FormOutcome::Result({
                if !self.admin(cx) {
                    return FormOutcome::Early(None);
                }
                let (name, pass) = (form.value(0), form.raw(1));
                let sec = if pass.is_empty() { iyakki_core_api::device_types::devctl::wifi::Security::Open } else { iyakki_core_api::device_types::devctl::wifi::Security::Wpa2 };
                match iyakki_core_api::device_types::devctl::wifi::Known::new(name.trim(), &pass, sec, true, iyakki_clock::now_unix()) {
                    Ok(k) => {
                        self.join_wifi(cx, k);
                        Ok("Working… this takes a few seconds.".to_string())
                    }
                    Err(e) => Err(e),
                }
            }),
            FormKind::Share(k) => FormOutcome::Result(self.shares_submit(cx, &k, form)),
            FormKind::HotspotWifi => FormOutcome::Result(self.hotspot_wifi_submit(cx, form)),
            FormKind::HotspotCountry => FormOutcome::Result(self.hotspot_country_submit(cx, form)),
            FormKind::Remote(k) => FormOutcome::Result(self.remote_submit(cx, &k, form)),
            FormKind::Sftp(k) => FormOutcome::Result(self.sftp_submit(cx, &k, form)),
            FormKind::Machine(k) => FormOutcome::Result(self.machine_submit(cx, &k, form)),
            FormKind::Fw(k) => FormOutcome::Result(self.fw_submit(cx, &k, form)),
            FormKind::SshTun(k) => FormOutcome::Result(self.tun_submit(cx, &k, form)),
            FormKind::Domains(k) => FormOutcome::Result(self.dom_submit(cx, &k, form)),
            FormKind::Vpn(k) => FormOutcome::Result(self.vpn_submit(cx, &k, form)),
            FormKind::BrowserHeader(k) => FormOutcome::Result(self.bh_submit(cx, &k, form)),
            FormKind::BrowserShort(add) => FormOutcome::Result(self.short_submit(cx, *add, form)),
            FormKind::BrowserSite => FormOutcome::Result(self.site_forget_submit(cx, form)),
            FormKind::BrowserSites(c) => FormOutcome::Result(self.browser_sites_submit(cx, *c, form)),
            FormKind::BrowserCert => FormOutcome::Result(self.browser_cert_submit(cx, form)),
            FormKind::BrowserBadAdd => FormOutcome::Result(self.browser_bad_add_submit(cx, form)),
            FormKind::BrowserBadRemove => FormOutcome::Result(self.browser_bad_remove_submit(cx, form)),
            FormKind::BrowserProxy => FormOutcome::Result(self.browser_proxy_submit(cx, form)),
            FormKind::BlockListUrl | FormKind::BlockListFile => FormOutcome::Result(self.submit_block_form(&kind, form, cx)),
            _ => FormOutcome::NotMine,
        }
    }
}

impl SettingsApp {
    pub(super) fn apply_pick_c(&mut self, cx: &AppCx, kind: PickKind, item: &str) -> bool {
        if matches!(kind, PickKind::CertView) {
            return true;
        }
        if matches!(kind, PickKind::FwService | PickKind::Wifi | PickKind::WifiForget | PickKind::MacMode(_)) && !self.admin(cx) {
            return true;
        }
        if kind == PickKind::FwService {
            self.fw_service_pick(cx, item);
            return true;
        }
        match kind {
            PickKind::Wifi => {
                let chosen = self.wifi_items.iter().find(|(t, _)| t == item).cloned();
                match chosen {
                    Some((_, Some(ap))) => {
                        if ap.security == iyakki_core_api::device_types::devctl::wifi::Security::Enterprise {
                            let f = Form::new(&format!("Join {} (enterprise)", ap.ssid))
                                .choice("Method", &["PEAP", "TTLS", "TLS (certificate)"], 0)
                                .text("User name", "", "your login at the school or company")
                                .secret("Password", "not for TLS")
                                .text("Outer name", "", "only if the network asks for one")
                                .text("Server certificate file", "", "full path of the authority file that checks the network (required)")
                                .text("Server name", "", "for example radius.example.edu (advised)")
                                .text("Your certificate file", "", "TLS only: full path")
                                .text("Your key file", "", "TLS only: full path")
                                .secret("Key password", "if the key file has one");
                            self.open_form(FormKind::WifiEnterprise(ap.ssid.clone()), f);
                            return true;
                        }
                        if ap.security == iyakki_core_api::device_types::devctl::wifi::Security::Wep {
                            let f = Form::new(&format!("Join {} (WEP: not safe)", ap.ssid)).secret("WEP key", "5 or 13 characters, or 10 or 26 hexadecimal digits").choice("WEP can be broken in minutes", &["No, do not join", "Yes, I understand"], 0);
                            self.open_form(FormKind::WifiWep(ap.ssid.clone()), f);
                            return true;
                        }
                        if ap.security == iyakki_core_api::device_types::devctl::wifi::Security::Open {
                            match iyakki_core_api::device_types::devctl::wifi::Known::new(&ap.ssid, "", ap.security, false, iyakki_clock::now_unix()) {
                                Ok(k) => self.join_wifi(cx, k),
                                Err(e) => self.fail(e),
                            }
                        } else {
                            let f = Form::new(&format!("Join {} ({})", ap.ssid, ap.security.label())).secret("Passphrase", "8 to 63 characters; only the key made from it is saved");
                            self.open_form(FormKind::WifiJoin(ap.ssid.clone(), ap.security), f);
                        }
                    }
                    Some((t, None)) if t.starts_with("Hidden") => {
                        let f = Form::new("Join a hidden WiFi network").text("Network name", "", "exactly as it is spelled").secret("Passphrase", "empty if the network is open");
                        self.open_form(FormKind::WifiHidden, f);
                    }
                    Some((t, None)) if t.starts_with("Forget") => {
                        let names: Vec<String> = self.wifi.known.list().into_iter().map(|k| k.ssid).collect();
                        self.modal = Some(Modal::Pick(PickKind::WifiForget, Picker::new("Forget which network?", names)));
                    }
                    _ => {}
                }
                return true;
            }
            PickKind::WifiForget => {
                match self.wifi.known.forget(item) {
                    Ok(true) => {
                        cx.audit.record(&cx.session.user, "settings.wifi.forget", item);
                        if let Some(iface) = self.wifi.iface() {
                            let _ = self.wifi.wifi.apply(&iface, &self.wifi.known.list());
                        }
                        self.ok(format!("Forgot {item}."));
                    }
                    Ok(false) => self.fail(format!("{item} was not saved.")),
                    Err(e) => self.fail(e),
                }
                return true;
            }
            PickKind::MacMode(k) => {
                let Some(mode) = iyakki_core_api::device_types::devctl::mac::MacMode::ALL.into_iter().find(|m| m.label() == item) else { return true };
                let mac = self.hw.mac().clone();
                let name = if k == iyakki_core_api::device_types::devctl::LinkKind::Wifi { "WiFi" } else { "Ethernet" };
                match mac.set_mode(Some(k), None, mode) {
                    Ok(_) => {
                        cx.audit.record(&cx.session.user, "settings.network.mac", &format!("{} {}", name.to_lowercase(), mode.key()));
                        self.hw.invalidate();
                        self.ok(format!("Private MAC address for {name}: {}. It applies on the next connection; press a to apply it now.", mode.key()));
                    }
                    Err(e) => self.fail(e),
                }
                return true;
            }
            _ => {}
        }
        false
    }
}

impl SettingsApp {
    pub(super) fn apply_confirm_c(&mut self, cx: &AppCx, kind: ConfirmKind) -> bool {
        if let ConfirmKind::Fleet(a) = kind {
            self.fleet_confirm(cx, a);
            return true;
        }
        if let ConfirmKind::Sftp(a) = kind {
            self.sftp_confirm(cx, a);
            return true;
        }
        if let ConfirmKind::MacApply(iface) = kind {
            self.mac_apply_now(cx, iface);
            return true;
        }
        if let ConfirmKind::ShareOpenFirewall = kind {
            self.fw_open_shares_here(cx);
            return true;
        }
        if let ConfirmKind::ShareStopAll = kind {
            self.shares_stop_all();
            return true;
        }
        if let ConfirmKind::MediaServerOn = kind {
            self.media_server_start(cx);
            return true;
        }
        if let ConfirmKind::Fw(act) = kind {
            self.fw_confirm(cx, act);
            return true;
        }
        if let ConfirmKind::VpnDelete(id) = kind {
            self.vpn_delete(cx, &id);
            return true;
        }
        if let ConfirmKind::DomainDelete(h, p) = &kind {
            self.dom_delete(cx, h, p);
            return true;
        }
        if let ConfirmKind::SshTunnelDelete(id) = kind {
            self.tun_delete(cx, &id);
            return true;
        }
        if let ConfirmKind::BrowserHeaderDelete(id) = kind {
            self.bh_delete(cx, &id);
            return true;
        }
        if matches!(kind, ConfirmKind::BrowserProtect | ConfirmKind::BrowserUnprotect | ConfirmKind::BrowserReset) {
            self.bp_confirm(cx, &kind);
            return true;
        }
        false
    }
}
