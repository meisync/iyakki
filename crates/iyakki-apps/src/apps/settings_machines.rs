//! Settings -> Remote access -> Machines for flows (F-FLOW-16): the other IYAKKI computers a flow's "Run on another IYAKKI" step may name.
//! A machine is a short name, the https address of its web service and the NAME of the vault entry that holds the sign-in token there
//! (never the token). This is the screen side of the assistant's `remote_machines`, `remote_machine_add` and `remote_machine_remove`:
//! the same `flows::machines` code, the same checks, the same audit action (`flow.remote_machine`). Keys: `M` adds (or replaces) one, `X`
//! forgets one. The form is the person's own act, so there is no second card; the chat tools ask first.

use super::*;
use iyakki_flowsx::machines::{vault_name, Machine, Machines, MAX_MACHINES};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MachineForm {
    Add,
    Forget,
}

/// The saved machines of this person: `<data>/flows/<user>/remote-machines.json`, the file the assistant's tools and the flow step use.
pub(crate) fn machines_of(cx: &AppCx) -> Machines {
    let user = if iyakki_sec::policy::valid_username(&cx.session.user) { cx.session.user.as_str() } else { "user" };
    Machines::new(&cx.data_dir().join("flows").join(user))
}

/// A certificate pasted into a one-line field comes back as the PEM the machines file wants: the lines of a pasted block run together
/// (new lines become spaces), so each block is cut out between its markers and its body is wrapped again at 64 characters. Anything else
/// (a key, other text) is left as it is for the checker to refuse without echoing it.
pub(crate) fn pem_from_pasted(text: &str) -> Result<String, String> {
    const B: &str = "-----BEGIN CERTIFICATE-----";
    const E: &str = "-----END CERTIFICATE-----";
    let t = text.trim();
    if t.is_empty() || t.contains("PRIVATE KEY") {
        return Ok(t.to_string());
    }
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find(B) {
        let after = &rest[i + B.len()..];
        let Some(j) = after.find(E) else { return Ok(t.to_string()) };
        let body: String = after[..j].chars().filter(|c| !c.is_whitespace()).collect();
        out.push_str(B);
        out.push('\n');
        for chunk in body.as_bytes().chunks(64) {
            out.push_str(&String::from_utf8_lossy(chunk));
            out.push('\n');
        }
        out.push_str(E);
        out.push('\n');
        rest = &after[j + E.len()..];
    }
    Ok(if out.is_empty() { t.to_string() } else { out })
}

impl SettingsApp {
    pub(super) fn machine_buttons(&self) -> Vec<Btn> {
        vec![("Add a machine for flows", TEXT, 'M'), ("Forget a machine", TEXT, 'X')]
    }

    pub(crate) fn machine_lines(&self, cx: &AppCx, lines: &mut Vec<(String, f32, [u8; 4], Face)>) {
        lines.push(("Machines for flows".into(), 14.0, TEXT, Face::Bold));
        let list: Vec<Machine> = machines_of(cx).list();
        if list.is_empty() {
            lines.push(("No machine is saved. M adds one: a flow step \"Run on another IYAKKI\" names a saved machine, never an address.".into(), 13.0, MUTED, Face::Sans));
        }
        for m in &list {
            lines.push((format!("{}  {}", m.name, m.url), 13.0, TEXT, Face::Mono));
            lines.push((format!("sign-in token: vault entry {}{}", m.token, if m.ca.is_empty() { "" } else { "  (private CA)" }), 12.0, DIM, Face::Sans));
        }
        lines.push((format!("Up to {MAX_MACHINES} machines. Only the name of the vault entry is kept here, never the token. Everything else is decided over there: its sign-in, its flow permission, its approvals."), 12.0, DIM, Face::Sans));
    }

    /// Keys of the machines part; true when the key was one of ours.
    pub(super) fn machine_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        let Key::Char(c) = k else { return false };
        match c {
            'M' => {
                self.open_form(
                    FormKind::Machine(MachineForm::Add),
                    Form::new("Add a machine for flows").text("Name", "", "letters, digits, - and _ (up to 40)").text("Address", "https://", "https, host and port only").text("Vault entry", "", "the NAME of the vault entry that holds the sign-in token there")
                        .text("CA certificate (optional)", "", "only when that machine's certificate comes from a private authority: paste the public PEM text (never a key)"),
                );
                true
            }
            'X' => {
                let first = machines_of(cx).list().first().map(|m| m.name.clone()).unwrap_or_default();
                self.open_form(FormKind::Machine(MachineForm::Forget), Form::new("Forget a machine").text("Name", &first, "flows that name it fail until it is saved again"));
                true
            }
            _ => false,
        }
    }

    pub(in crate::apps) fn machine_submit(&mut self, cx: &AppCx, k: &MachineForm, form: &mut Form) -> Result<String, String> {
        let machines = machines_of(cx);
        match k {
            MachineForm::Add => {
                let (name, url, token) = (form.value(0).trim().to_string(), form.value(1).trim().to_string(), form.value(2));
                if vault_name(&token).is_none() {
                    return Err("give the NAME of the vault entry that holds the token (never the token itself)".into());
                }
                let ca = pem_from_pasted(&form.value(3))?;
                let m = machines.add_with_ca(&name, &url, &token, &ca)?;
                cx.audit.record(&cx.session.user, "flow.remote_machine", &format!("saved {} {}{}", m.name, m.url, if m.ca.is_empty() { "" } else { " (private CA)" }));
                Ok(format!("Saved the machine {} at {}{}.", m.name, m.url, if m.ca.is_empty() { "" } else { " with its private authority's certificate" }))
            }
            MachineForm::Forget => {
                let name = form.value(0).trim().to_string();
                if machines.remove(&name)? {
                    cx.audit.record(&cx.session.user, "flow.remote_machine", &format!("removed {name}"));
                    Ok(format!("Forgot the machine {name}."))
                } else {
                    Err("No machine has that name.".into())
                }
            }
        }
    }
}
