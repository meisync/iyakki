//! Settings: the storage page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::system_port::StoragePort;
use super::*;

impl SettingsApp {
    pub(super) fn handle_storage(&mut self, k: &Key, p: &dyn StoragePort) {
        match k {
            Key::Up => self.move_sel(-1, self.devices.len()),
            Key::Down => self.move_sel(1, self.devices.len()),
            Key::Char('r') => {
                self.refresh_devices();
                self.ok("Disks rescanned.");
            }
            Key::Char('d') if p.allows_disks() => self.hw.ask_open("disks"),
            Key::Char('o') if p.allows_disks() => {
                let home = p.home_root(&self.storage.data_dir);
                let on = !p.disks_signout_pref(&home);
                match p.disks_set_signout_pref(&home, on) {
                    Ok(()) => self.ok(if on { "Your drives will be closed when you sign out." } else { "Signing out will leave your drives open (and warn you)." }),
                    Err(e) => self.fail(format!("Could not save the choice: {e}")),
                }
            }
            // compatible USB mode (no UAS) for the next start: slower, steadier (docs/STORAGE-LOSS.md)
            Key::Char('u') if self.admin(p) => {
                let snap = p.usb_loss_snapshot();
                let Some(link) = snap.link.clone() else { return self.fail("The drive IYAKKI runs from is not a USB drive (or its port could not be read), so there is nothing to change.") };
                match p.set_usb_compat(!snap.compat, &link.id) {
                    Ok(m) => {
                        p.audit("storage.usbcompat", if snap.compat { "off" } else { "on" });
                        self.ok(m)
                    }
                    Err(e) => self.fail(format!("Could not change the USB mode: {e}")),
                }
            }
            Key::Char('b') if self.admin(p) => {
                if p.allows_power() {
                    self.modal = Some(Modal::Confirm(ConfirmKind::Reboot, "Restart this machine now?".into()));
                } else {
                    self.fail("Your account may not restart the machine.");
                }
            }
            Key::Char('n') if self.admin(p) && self.storage.virtual_disks.is_some() => {
                self.open_form(FormKind::NewVirtualDisk, Form::new("New virtual disk").text("Size (for example 8G, 512M)", "8G", "8G"));
            }
            Key::Char('x') if self.admin(p) && self.storage.virtual_disks.is_some() => {
                let Some(dev) = self.devices.get(self.sel()).cloned() else { return };
                if dev.parent.is_some() {
                    return self.fail("Pick the disk itself, not one of its partitions.");
                }
                let f = Form::new(&format!("Delete the virtual disk {} and everything on it?", dev.name)).text(&format!("Type {} to confirm", dev.name), "", "the image file is removed");
                self.open_form(FormKind::RemoveVirtualDisk(dev.name.clone()), f);
            }
            Key::Char('m') if self.admin(p) => {
                let Some(dev) = self.devices.get(self.sel()).cloned() else { return };
                match iyakki_core_api::device_types::storage::eligibility(&dev, &self.devices) {
                    Err(why) => self.fail(format!("Cannot use it: {why}.")),
                    Ok(what) => {
                        let mut f = Form::new(&format!("Erase /dev/{} and use it for IYAKKI data?", dev.name)).text(&format!("Type {} to confirm", dev.name), "", "everything on this disk will be lost");
                        if let Some(w) = &what {
                            f = f.text(&format!("It holds {w}. Type ERASE"), "", "ERASE");
                        }
                        self.open_form(FormKind::Persist(dev.name.clone(), what.is_some()), f);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn render_storage(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn StoragePort) {
        use iyakki_core_api::device_types::storage::DataLocation;
        let loc = iyakki_core_api::device_types::storage::data_location(&self.storage.roots, &self.storage.data_dir);
        let (text, col) = match &loc {
            DataLocation::Disk(d) => (format!("Saved to disk ({d}). Your accounts, settings, vaults, documents and chats survive power-off."), OK),
            DataLocation::Memory => ("Kept in RAM. Everything is forgotten at power-off. Pick a disk below to keep your data.".to_string(), crate::fx::AMBER),
            DataLocation::Other(o) => (format!("Stored on a {o} volume."), MUTED),
        };
        for (i, l) in g.wrap(15.0 * s, &text, Face::Bold, a.w).iter().take(2).enumerate() {
            g.text(a.x, a.y + i as f32 * 21.0 * s, 15.0 * s, col, l, Face::Bold);
        }
        if let Some((free, total)) = iyakki_core_api::device_types::storage::space(&self.storage.data_dir) {
            g.text(a.x, a.y + 46.0 * s, 13.0 * s, MUTED, &format!("{} free of {}", iyakki_core_api::device_types::storage::human(free), iyakki_core_api::device_types::storage::human(total)), Face::Sans);
        }
        let mut y = a.y + 76.0 * s;
        // how the boot drive is connected (speed, port, hub) and the compatible USB mode (docs/STORAGE-LOSS.md)
        {
            let snap = p.usb_loss_snapshot();
            if let Some(l) = &snap.link {
                let name = if l.name.is_empty() { "The drive".to_string() } else { l.name.clone() };
                let driver = if l.driver == "uas" { "UAS (fast)" } else if l.driver.is_empty() { "unknown driver" } else { "usb-storage (compatible)" };
                let line = format!("{name} runs at {} on USB port {}{}, {driver}.", l.speed_text(), l.port, if l.behind_hub { " behind a hub" } else { "" });
                let col = if l.slow() || l.behind_hub { crate::fx::AMBER } else { MUTED };
                for t in g.wrap(13.0 * s, &line, Face::Sans, a.w).iter().take(2) {
                    g.text(a.x, y, 13.0 * s, col, t, Face::Sans);
                    y += 18.0 * s;
                }
                let advice = iyakki_core_api::device_types::storage::loss::link_advice(Some(l));
                if !advice.is_empty() {
                    for t in g.wrap(13.0 * s, &advice, Face::Sans, a.w).iter().take(3) {
                        g.text(a.x, y, 13.0 * s, crate::fx::AMBER, t, Face::Sans);
                        y += 18.0 * s;
                    }
                }
                let mode = if snap.compat { "Compatible USB mode (no UAS) is ON: slower, steadier." } else { "Compatible USB mode (no UAS): off. Turn it on if the drive keeps disconnecting; it is slower." };
                let t = g.fit(13.0 * s, &format!("{mode} Press u to switch (next start)."), Face::Sans, a.w);
                g.text(a.x, y, 13.0 * s, ACCENT, &t, Face::Sans);
                g.hit(R::new(a.x, y - 2.0 * s, a.w, 20.0 * s), Event::Key(Key::Char('u')), "Compatible USB mode");
                y += 24.0 * s;
            }
            if !snap.state.healthy() {
                let t = iyakki_core_api::device_types::storage::loss::unavailable_text(&snap);
                for l in g.wrap(13.0 * s, &t, Face::Bold, a.w).iter().take(2) {
                    g.text(a.x, y, 13.0 * s, ERR, l, Face::Bold);
                    y += 18.0 * s;
                }
                y += 4.0 * s;
            }
        }
        // the drives that are open now, and the way to DISKS (mount, unmount, eject, partitions)
        if p.allows_disks() {
            let home = p.home_root(&self.storage.data_dir);
            let mine = p.disks_media_dir(&home) + "/";
            let open: Vec<String> = self.devices.iter().filter_map(|d| d.mounted_at.as_ref().filter(|m| m.starts_with(&mine)).map(|m| format!("{} at {m}", d.name))).collect();
            let sw = if p.disks_signout_pref(&home) { "closed at sign-out (o: leave open)" } else { "left open at sign-out (o: close them)" };
            let t = if open.is_empty() { format!("No USB drive or card is open in your home. Press d for DISKS. Drives are {sw}.") } else { format!("Open in your home: {}. Press d for DISKS. Drives are {sw}.", open.join(", ")) };
            let t = g.fit(13.0 * s, &t, Face::Sans, a.w);
            g.text(a.x, y, 13.0 * s, ACCENT, &t, Face::Sans);
            y += 22.0 * s;
        }
        if self.storage.virtual_disks.is_some() {
            let note = "Virtual disks: this window only shows disks it makes for itself (files in its own folder), so nothing here can touch your computer's drives.";
            for l in g.wrap(13.0 * s, note, Face::Sans, a.w).iter().take(2) {
                g.text(a.x, y, 13.0 * s, ACCENT, l, Face::Sans);
                y += 18.0 * s;
            }
            y += 6.0 * s;
        }
        if self.devices.is_empty() {
            let none = if self.storage.virtual_disks.is_some() { "No virtual disks yet. Press n to make one." } else { "No disks found. Attach a USB drive or disk and press r." };
            g.text(a.x, y, 15.0 * s, DIM, none, Face::Sans);
        }
        for (i, d) in self.devices.iter().enumerate() {
            if y > a.y + a.h - 58.0 * s {
                break;
            }
            let indent = if d.parent.is_some() { 24.0 * s } else { 0.0 };
            let verdict = match iyakki_core_api::device_types::storage::eligibility(d, &self.devices) {
                Err(why) => why,
                Ok(None) => "empty — can be used".to_string(),
                Ok(Some(what)) => format!("holds {what}"),
            };
            let sub = format!("{} · {}", if d.model.is_empty() { d.content.describe() } else { format!("{} · {}", d.model, d.content.describe()) }, verdict);
            let size = d.human_size();
            let mut chips: Vec<(&str, [u8; 4])> = vec![(size.as_str(), MUTED)];
            if d.removable {
                chips.push(("removable", ACCENT));
            }
            if d.content == iyakki_core_api::device_types::storage::Content::Iyakki {
                chips.push(("IYAKKI data", OK));
            }
            if d.mounted_at.is_some() {
                chips.push(("in use", ERR));
            }
            let on = i == self.sel();
            row(g, &mut self.tip, i, a.x + indent, y, a.w - indent, 58.0 * s, s, on, &format!("/dev/{}", d.name), &sub, &chips);
            y += 64.0 * s;
        }
    }
}
