//! Lane ST-D: the submit arms of its forms (moved out of settings.rs, S2 of SETTINGS-EXTRACTION-PLAN.md; no behaviour change).

use super::*;

impl SettingsApp {
    /// The forms of lane ST-D: NotMine when the kind is another lane's.
    pub(super) fn submit_form_d(&mut self, cx: &AppCx, kind: &FormKind, form: &mut Form) -> FormOutcome {
        match kind {
            FormKind::Dc(f) => FormOutcome::Result(self.dc_submit(cx, &f, form)),
            FormKind::Git(k) => FormOutcome::Result(self.git_submit(cx, &k, form)),
            FormKind::HomeGroup(id) => FormOutcome::Result(self.hl_submit(cx, id, form)),
            FormKind::View(id) => FormOutcome::Result(self.vw_submit(cx, id, form)),
            FormKind::ViewKeys => FormOutcome::Result(self.vw_keys_submit(cx, form)),
            _ => FormOutcome::NotMine,
        }
    }
}

impl SettingsApp {
    pub(super) fn apply_pick_d(&mut self, cx: &AppCx, kind: PickKind, item: &str) -> bool {
        // the home layout is everyone's own, not an administrator's setting
        if let PickKind::HomeMove(key) = &kind {
            self.hl_pick(cx, key, item);
            return true;
        }
        if matches!(kind, PickKind::ViewTemplate) {
            self.vw_pick(cx, item);
            return true;
        }
        if matches!(kind, PickKind::SoundOutput | PickKind::BluetoothSink) && !self.admin(cx) {
            return true;
        }
        if kind == PickKind::SoundOutput {
            self.sound_pick(cx, item);
            return true;
        }
        if kind == PickKind::BluetoothSink {
            // the item reads `Name  [address]`: the address is what is sent on
            let address = item.rsplit_once('[').and_then(|(_, a)| a.strip_suffix(']')).unwrap_or("").to_string();
            let name = item.split("  [").next().unwrap_or(item).to_string();
            self.modal = Some(Modal::Confirm(ConfirmKind::BluetoothSound(address), format!("Send all of this computer's sound to {name}? Its own speakers go silent until you stop it. Press y to start.")));
            return true;
        }
        false
    }
}

impl SettingsApp {
    pub(super) fn apply_confirm_d(&mut self, cx: &AppCx, kind: ConfirmKind) -> bool {
        if let ConfirmKind::BluetoothSound(address) = &kind {
            self.sound_bluetooth_start(cx, address);
            return true;
        }
        if matches!(kind, ConfirmKind::HomeDelete(_) | ConfirmKind::HomeReset | ConfirmKind::HomeDefaultSet | ConfirmKind::HomeDefaultClear) {
            self.hl_confirm(cx, &kind);
            return true;
        }
        if let ConfirmKind::ViewDelete(id) = kind {
            self.vw_confirm(cx, id);
            return true;
        }
        if let ConfirmKind::Dc(a) = kind {
            self.dc_confirm(cx, a);
            return true;
        }
        if let ConfirmKind::SoundHwWorks(_) = kind {
            self.sound_hw_answer(cx, &kind, true);
            return true;
        }
        if let ConfirmKind::T2Mic = kind {
            self.sound_t2_mic_apply(cx, true);
            return true;
        }
        false
    }
}
