//! The wizard engine (F-CORE-24): runs a [`WizardSpec`] of an app or a bundle step by step. It is a plain state machine with no screen: a
//! screen (or a chat tool) asks [`Run::view`] what to show, hands answers to [`Run::submit`] and calls [`Run::advance`] to let the
//! automatic steps (checks, migrations) run. Everything that touches the machine goes through [`Env`], so the engine is tested without one.
//!
//! * A wizard this IYAKKI cannot run (a newer `wizard_version`, an unknown trigger, step kind, field kind, rule or check) is refused
//!   before it starts, with the words of [`WizardSpec::check_runnable`].
//! * Answers are checked against the field kind, `required` and the rules; the errors name the field and say what is wrong.
//! * Secret fields are kept apart: they are never in [`Run::answers_redacted`] and never in the log lines.
//! * The wizard runs once: a failed check or migration ends it as `Failed` with the sentence; nothing is retried by itself.

use std::collections::BTreeMap;

use iyakki_core_api::wizard::{Field, FieldKind, Step, TriggerKind, WizardSpec};
use serde_json::Value;

/// What the engine needs from the machine.
pub trait Env {
    /// Run a check of the spec (`net.reach`, `path.readable`, `path.writable`, `capability.held`) with its param.
    fn check(&self, check: &str, param: &str) -> Result<(), String>;
    /// Run a migration of the app (by reference) from a version to a version. Returns a short sentence of what it did.
    fn migrate(&self, reference: &str, from: Option<&str>, to: Option<&str>, answers: &BTreeMap<String, Value>) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldView {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    pub help: String,
    /// The value to show first: what the person gave before, else the default (never for a secret).
    pub value: Option<Value>,
    pub choices: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Form { step: String, title: String, fields: Vec<FieldView> },
    /// An automatic step is next: call [`Run::advance`].
    Working { step: String, title: String },
    Summary { title: String, text: String },
    Finished(State),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

pub struct Run {
    spec: WizardSpec,
    at: usize,
    answers: BTreeMap<String, Value>,
    secrets: Vec<String>,
    state: State,
    log: Vec<String>,
    from_version: Option<String>,
}

fn secret_ids(spec: &WizardSpec) -> Vec<String> {
    let mut v = vec![];
    for s in &spec.steps {
        if let Step::Form(f) = s {
            v.extend(f.fields.iter().filter(|x| x.kind() == FieldKind::Secret).map(|x| x.id.clone()));
        }
    }
    v
}

impl Run {
    /// Begin a wizard for `trigger` ("on_install", "on_update", "reconfigure", "on_remove"). For an update, `installed` is the version the
    /// app is updated from and the spec's `from` range must match it.
    pub fn start(spec: WizardSpec, trigger: &str, installed: Option<&str>) -> Result<Run, String> {
        spec.check_runnable()?;
        let kind = spec.trigger.kind();
        let wanted = match trigger {
            "on_install" => TriggerKind::OnInstall,
            "on_update" => TriggerKind::OnUpdate,
            "reconfigure" => TriggerKind::Reconfigure,
            "on_remove" => TriggerKind::OnRemove,
            other => return Err(format!("{other:?} is not a wizard trigger")),
        };
        if kind != wanted {
            return Err(format!("this wizard runs on {:?}, not on {trigger}", spec.trigger.on));
        }
        if wanted == TriggerKind::OnUpdate && !installed.is_some_and(|v| spec.trigger.matches_update_from(v)) {
            return Err("this wizard is not for the version being updated from".into());
        }
        let secrets = secret_ids(&spec);
        Ok(Run { spec, at: 0, answers: BTreeMap::new(), secrets, state: State::Running, log: vec![], from_version: installed.map(str::to_string) })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn id(&self) -> &str {
        &self.spec.id
    }

    /// What was done so far, one plain line per step (no secret values).
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// The answers without the secret fields.
    pub fn answers_redacted(&self) -> BTreeMap<String, Value> {
        self.answers.iter().filter(|(k, _)| !self.secrets.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// All answers, for the one place that applies them (the installer). Secrets go to the vault from here, not to a file.
    pub fn take_answers(self) -> BTreeMap<String, Value> {
        self.answers
    }

    /// One plain line per automatic step that `advance` would run next (until a form, the summary or the end): the card of the chat tool.
    pub fn upcoming(&self) -> Vec<String> {
        let mut v = vec![];
        for s in self.spec.steps.iter().skip(self.at) {
            match s {
                Step::Check(c) => v.push(format!("check {} {}", c.check, c.param.clone().unwrap_or_default()).trim_end().to_string()),
                Step::Migration(m) => v.push(format!("migration {}{}", m.migration, m.to.as_ref().map_or(String::new(), |t| format!(" to {t}")))),
                _ => break,
            }
        }
        v
    }

    pub fn view(&self) -> View {
        if self.state != State::Running {
            return View::Finished(self.state.clone());
        }
        match self.spec.steps.get(self.at) {
            None => View::Finished(State::Done),
            Some(Step::Form(f)) => View::Form {
                step: f.id.clone(),
                title: f.title.clone(),
                fields: f
                    .fields
                    .iter()
                    .map(|x| FieldView {
                        id: x.id.clone(),
                        label: x.label.clone(),
                        kind: x.kind.clone(),
                        required: x.required,
                        help: x.help.clone(),
                        value: if x.kind() == FieldKind::Secret { None } else { self.answers.get(&x.id).cloned().or_else(|| x.default.clone()) },
                        choices: x.choices.iter().map(|c| (c.value.clone(), c.label.clone())).collect(),
                    })
                    .collect(),
            },
            Some(Step::Summary(s)) => View::Summary { title: s.title.clone(), text: s.text.clone() },
            Some(other) => View::Working { step: other.id(), title: match other { Step::Check(c) => c.title.clone(), Step::Migration(m) => m.title.clone(), _ => String::new() } },
        }
    }

    /// Answers for the form step that is showing. A field that is not in `values` keeps its earlier answer or its default. On any error
    /// nothing is stored and the step stays.
    pub fn submit(&mut self, values: &BTreeMap<String, Value>) -> Result<(), Vec<FieldError>> {
        let Some(Step::Form(f)) = self.spec.steps.get(self.at).filter(|_| self.state == State::Running) else {
            return Err(vec![FieldError { field: String::new(), message: "there is no form to answer right now".into() }]);
        };
        if let Some(k) = values.keys().find(|k| !f.fields.iter().any(|x| &x.id == *k)) {
            return Err(vec![FieldError { field: k.clone(), message: "this step has no such field".into() }]);
        }
        let mut errs = vec![];
        let mut next = BTreeMap::new();
        for fl in &f.fields {
            let v = values.get(&fl.id).cloned().or_else(|| self.answers.get(&fl.id).cloned()).or_else(|| fl.default.clone());
            match check_field(fl, v.as_ref()) {
                Ok(Some(v)) => {
                    next.insert(fl.id.clone(), v);
                }
                Ok(None) => {}
                Err(m) => errs.push(FieldError { field: fl.id.clone(), message: m }),
            }
        }
        if !errs.is_empty() {
            return Err(errs);
        }
        self.answers.extend(next);
        self.log.push(format!("{}: answered ({} field(s))", f.id, f.fields.len()));
        self.at += 1;
        Ok(())
    }

    /// Run the automatic steps until a form or the summary is next, the wizard fails, or it is over.
    pub fn advance(&mut self, env: &dyn Env) {
        while self.state == State::Running {
            match self.spec.steps.get(self.at).cloned() {
                None => self.state = State::Done,
                Some(Step::Check(c)) => {
                    let param = c.param.clone().unwrap_or_default();
                    match env.check(&c.check, &param) {
                        Ok(()) => {
                            self.log.push(format!("{}: {} passed", c.id, c.check));
                            self.at += 1;
                        }
                        Err(e) => {
                            self.log.push(format!("{}: {} failed", c.id, c.check));
                            self.state = State::Failed(format!("{}: {e}", if c.title.is_empty() { &c.id } else { &c.title }));
                        }
                    }
                }
                Some(Step::Migration(m)) => {
                    let from = m.from.clone().or_else(|| self.from_version.clone());
                    match env.migrate(&m.migration, from.as_deref(), m.to.as_deref(), &self.answers_redacted()) {
                        Ok(said) => {
                            self.log.push(format!("{}: {said}", m.id));
                            self.at += 1;
                        }
                        Err(e) => {
                            self.log.push(format!("{}: failed", m.id));
                            self.state = State::Failed(format!("{}: {e}", if m.title.is_empty() { &m.id } else { &m.title }));
                        }
                    }
                }
                Some(Step::Summary(_)) | Some(Step::Form(_)) => return,
                Some(Step::Unknown(_)) => self.state = State::Failed("a step of an unknown kind".into()),
            }
        }
    }

    /// The person read the summary (or there is none): the wizard is over.
    pub fn finish(&mut self) {
        if self.state == State::Running && matches!(self.spec.steps.get(self.at), Some(Step::Summary(_)) | None) {
            self.log.push("finished".into());
            self.at = self.spec.steps.len();
            self.state = State::Done;
        }
    }

    pub fn cancel(&mut self) {
        if self.state == State::Running {
            self.log.push("cancelled".into());
            self.state = State::Cancelled;
        }
    }
}

/// One field against its kind and rules. `Ok(None)` is an optional field left empty.
fn check_field(fl: &Field, v: Option<&Value>) -> Result<Option<Value>, String> {
    let empty = match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        _ => false,
    };
    if empty {
        return if fl.required && fl.kind() != FieldKind::Toggle { Err("this is needed".into()) } else { Ok(None) };
    }
    let v = v.cloned().unwrap_or(Value::Null);
    match fl.kind() {
        FieldKind::Toggle => {
            if !v.is_boolean() {
                return Err("choose on or off".into());
            }
            return Ok(Some(v));
        }
        FieldKind::Unknown(k) => return Err(format!("unknown field kind {k:?}")),
        _ => {}
    }
    let Some(t) = v.as_str() else { return Err("this takes text".into()) };
    if t.chars().any(|c| c.is_control()) {
        return Err("no control characters".into());
    }
    match fl.kind() {
        FieldKind::Choice if !fl.choices.iter().any(|c| c.value == t) => return Err("pick one of the choices".into()),
        FieldKind::Path if !t.starts_with('/') || t.split('/').any(|s| s == "..") => return Err("an absolute path without ..".into()),
        _ => {}
    }
    for r in &fl.rules {
        let ok = match r.rule.as_str() {
            "min_len" => r.value.as_ref().and_then(Value::as_u64).is_none_or(|n| t.chars().count() as u64 >= n),
            "max_len" => r.value.as_ref().and_then(Value::as_u64).is_none_or(|n| t.chars().count() as u64 <= n),
            "no_whitespace" => !t.chars().any(char::is_whitespace),
            "absolute_path" => t.starts_with('/') && !t.split('/').any(|s| s == ".."),
            "one_of" => r.value.as_ref().and_then(Value::as_array).is_none_or(|a| a.iter().any(|x| x.as_str() == Some(t))),
            _ => false,
        };
        if !ok {
            return Err(if r.message.is_empty() { plain_rule(&r.rule, r.value.as_ref()) } else { r.message.clone() });
        }
    }
    Ok(Some(v))
}

fn plain_rule(rule: &str, v: Option<&Value>) -> String {
    let n = v.and_then(Value::as_u64).unwrap_or(0);
    match rule {
        "min_len" => format!("at least {n} characters"),
        "max_len" => format!("at most {n} characters"),
        "no_whitespace" => "no spaces".into(),
        "absolute_path" => "an absolute path without ..".into(),
        "one_of" => "not one of the allowed values".into(),
        _ => "not allowed".into(),
    }
}

#[cfg(test)]
#[path = "wizardrun_tests.rs"]
mod tests;
