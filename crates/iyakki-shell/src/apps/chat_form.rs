//! The assistant's `ask_form` in the chat (FLOWS): the form for missing input, drawn with the shared modal form so that Tab, Shift+Tab,
//! Enter (submit), Esc (cancel), the mouse and masked secret fields all work as in every other dialog. The filled values go back as
//! the next message; a secret never does: it is stored in the person's vault and only its name is sent. Nothing about a secret value
//! reaches the transcript, the audit line or the log.

use iyakki_agent::flows::askform::{FormSpec, Kind, CANCELLED};

use crate::apps::widgets::{FieldKind, Form, FormEvent};
use super::{AppCx, ChatApp};
use crate::ui::Event;
use crate::filepicker::PathKind;

const NONE: &str = "(none)";

/// The modal form for a spec.
pub(super) fn build(spec: &FormSpec) -> Form {
    let mut f = Form::new(&spec.title);
    for fld in &spec.fields {
        let label = format!("{}{}", fld.label, if fld.required { " *" } else { "" });
        let d = fld.default.clone().unwrap_or_default();
        match fld.kind {
            Kind::Text | Kind::Number => f = f.text(&label, &d, &fld.help),
            Kind::Path => f = f.path(&label, &d, &fld.help, PathKind::File, &[]),
            Kind::Secret => f = f.secret(&label, if fld.help.is_empty() { "stored in your vault, never shown to the assistant" } else { &fld.help }),
            Kind::Choice | Kind::YesNo => {
                let mut opts: Vec<String> = if fld.kind == Kind::YesNo { vec!["Yes".into(), "No".into()] } else { fld.choices.clone() };
                if !fld.required {
                    opts.insert(0, NONE.into());
                }
                let want = match (fld.kind, d.as_str()) {
                    (Kind::YesNo, "yes") => "Yes".to_string(),
                    (Kind::YesNo, "no") => "No".to_string(),
                    _ => d.clone(),
                };
                let sel = opts.iter().position(|o| *o == want).unwrap_or(0);
                let refs: Vec<&str> = opts.iter().map(String::as_str).collect();
                f = f.choice(&label, &refs, sel);
            }
        }
    }
    f
}

/// The values the form holds, one per field, in the spec's own terms (yes/no as `yes`/`no`, no choice as empty).
pub(super) fn values(spec: &FormSpec, form: &Form) -> Vec<String> {
    spec.fields
        .iter()
        .enumerate()
        .map(|(i, fld)| match fld.kind {
            Kind::Secret => form.raw(i),
            Kind::Choice | Kind::YesNo => {
                let opts = &form.fields[i].kind;
                let FieldKind::Choice(o) = opts else { return String::new() };
                let v = o.get(form.choice_idx(i)).cloned().unwrap_or_default();
                match (fld.kind, v.as_str()) {
                    (_, NONE) => String::new(),
                    (Kind::YesNo, "Yes") => "yes".into(),
                    (Kind::YesNo, "No") => "no".into(),
                    _ => v,
                }
            }
            _ => form.value(i),
        })
        .collect()
}

impl ChatApp {
    /// A form for missing input is open (the chat takes its keys and draws it over the conversation).
    pub fn form_open(&self) -> bool {
        self.askform.is_some()
    }

    /// The titles of the open form's fields (for tests and the screen reader line).
    pub fn form_labels(&self) -> Vec<String> {
        self.askform.as_ref().map(|(_, f)| f.fields.iter().map(|x| x.label.clone()).collect()).unwrap_or_default()
    }

    /// The message under the open form (what is wrong), if any.
    pub fn form_error(&self) -> Option<String> {
        self.askform.as_ref().and_then(|(_, f)| f.error.clone())
    }

    /// Open the form the assistant asked for (a second request while one is open replaces nothing).
    pub(super) fn open_ask_form(&mut self, spec: FormSpec) {
        if self.askform.is_none() {
            let f = build(&spec);
            self.askform = Some((spec, f));
        }
    }

    pub(super) fn ask_form_key(&mut self, ev: &Event, cx: &AppCx) {
        let Some((spec, form)) = self.askform.as_mut() else { return };
        match form.handle_event(ev) {
            FormEvent::Cancel => {
                self.askform = None;
                self.send_form_answer(cx, CANCELLED.to_string());
            }
            FormEvent::Submit => {
                let vals = values(spec, form);
                if let Err((i, msg)) = spec.check(&vals) {
                    form.focus = i;
                    form.error = Some(msg);
                    return;
                }
                // a secret goes to the vault; only its name travels on
                let mut names: Vec<Option<String>> = vec![None; spec.fields.len()];
                for (i, f) in spec.fields.iter().enumerate() {
                    if f.kind == Kind::Secret && !vals[i].is_empty() {
                        let n = spec.secret_name(i);
                        match cx.services.user_set(&cx.session.user, &n, &vals[i]) {
                            Ok(_) => {
                                cx.audit.record(&cx.session.user, "vault.user.set", &n);
                                names[i] = Some(n);
                            }
                            Err(iyakki_agent::vault::VaultError::Locked) => {
                                form.focus = i;
                                form.error = Some("your personal vault is locked, so a secret cannot be stored now".into());
                                return;
                            }
                            Err(e) => {
                                form.focus = i;
                                form.error = Some(e.to_string());
                                return;
                            }
                        }
                    }
                }
                let text = spec.answer_text(&vals, &names);
                cx.audit.record(&cx.session.user, "chat.form.submitted", &format!("{} fields", spec.fields.len()));
                self.askform = None;
                self.send_form_answer(cx, text);
            }
            _ => {}
        }
    }

    fn send_form_answer(&mut self, cx: &AppCx, text: String) {
        let draft = self.input.text.clone();
        self.input.set(&text);
        self.view.focus = None;
        self.send(cx);
        if !draft.is_empty() {
            self.input.set(&draft);
        }
    }
}
