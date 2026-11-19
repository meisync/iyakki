//! The wizard spec: the declarative install, update, reconfigure and remove wizards an app (or a bundle member) brings in `app.json`
//! (docs/development/CORE-ACCESS-MODEL.md section 6e). Types and validation only; the engine that draws and runs the steps, takes the
//! snapshot and runs migrations is not built.
//!
//! Versioned by `wizard_version` and following the compatibility charter: unknown fields are kept and written back, a step kind, field kind,
//! rule or trigger this IYAKKI does not know is data (parsed, kept, validated only for size) and is refused at RUN time by
//! [`WizardSpec::check_runnable`], never at parse time. Serde only, so it builds for wasm32.

use std::collections::BTreeMap;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::appcaps::{self, CapabilityRequest};
use crate::bundle::{id_ok, parse_version, rel_path_ok, VersionRange};

/// The wizard spec format this IYAKKI can run.
pub const WIZARD_VERSION: u32 = 1;

pub const MAX_WIZARDS: usize = 8;
pub const MAX_STEPS: usize = 12;
pub const MAX_FIELDS: usize = 12;
pub const MAX_RULES: usize = 6;
pub const MAX_CHOICES: usize = 24;
pub const MAX_LABEL: usize = 80;
pub const MAX_TEXT: usize = 600;
pub const MAX_DEFAULT: usize = 200;
pub const MAX_REF: usize = 100;
pub const MAX_RULE_NUMBER: u64 = 10_000;

fn one() -> u32 {
    1
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// When a wizard runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    OnInstall,
    /// With an optional `from` version range of the installed version.
    OnUpdate,
    Reconfigure,
    OnRemove,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub on: String,
    /// `on_update` only: the installed versions this wizard is for, like `<2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Trigger {
    pub fn kind(&self) -> TriggerKind {
        match self.on.as_str() {
            "on_install" => TriggerKind::OnInstall,
            "on_update" => TriggerKind::OnUpdate,
            "reconfigure" => TriggerKind::Reconfigure,
            "on_remove" => TriggerKind::OnRemove,
            other => TriggerKind::Unknown(other.to_string()),
        }
    }

    /// Whether an `on_update` wizard applies when going from `installed` (a version). Other triggers answer false.
    pub fn matches_update_from(&self, installed: &str) -> bool {
        if self.kind() != TriggerKind::OnUpdate {
            return false;
        }
        match (&self.from, parse_version(installed)) {
            (None, _) => true,
            (Some(r), Some(v)) => VersionRange::parse(r).is_ok_and(|r| r.satisfied_by(v)),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Choice,
    Secret,
    Path,
    Toggle,
    Unknown(String),
}

impl FieldKind {
    pub fn of(s: &str) -> FieldKind {
        match s {
            "text" => FieldKind::Text,
            "choice" => FieldKind::Choice,
            "secret" => FieldKind::Secret,
            "path" => FieldKind::Path,
            "toggle" => FieldKind::Toggle,
            other => FieldKind::Unknown(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

/// A validation rule of a field: `min_len`, `max_len` (with a number), `no_whitespace`, `absolute_path`, `one_of` (with a list of text).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Shown when the rule fails; the shell has plain words for the known rules when this is empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// A secret never has one (secrets go to the vault, not into a manifest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Field {
    pub fn kind(&self) -> FieldKind {
        FieldKind::of(&self.kind)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormStep {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub fields: Vec<Field>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A test the core runs: `net.reach` (param host or host:port; needs `net.connect`), `path.readable` (needs `files.read`), `path.writable`
/// (needs `files.write`), `capability.held` (param a capability name). A check the core does not know is refused at run time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStep {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub check: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A data migration by reference (a name or a file of the app); the engine is not built. `from` and `to` are versions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationStep {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub migration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryStep {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One step. A kind nobody here knows is `Unknown(raw)`, kept as it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Form(FormStep),
    Check(CheckStep),
    Migration(MigrationStep),
    Summary(SummaryStep),
    Unknown(Value),
}

impl Step {
    pub fn kind_name(&self) -> String {
        match self {
            Step::Form(_) => "form".into(),
            Step::Check(_) => "check".into(),
            Step::Migration(_) => "migration".into(),
            Step::Summary(_) => "summary".into(),
            Step::Unknown(v) => v.get("kind").and_then(Value::as_str).unwrap_or("?").to_string(),
        }
    }

    pub fn id(&self) -> String {
        match self {
            Step::Form(s) => s.id.clone(),
            Step::Check(s) => s.id.clone(),
            Step::Migration(s) => s.id.clone(),
            Step::Summary(s) => s.id.clone(),
            Step::Unknown(v) => v.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Step, D::Error> {
        let v = Value::deserialize(d)?;
        let kind = v.get("kind").and_then(Value::as_str).ok_or_else(|| D::Error::custom("a step needs a \"kind\""))?.to_string();
        let mut body = v.clone();
        if let Some(o) = body.as_object_mut() {
            o.remove("kind");
        }
        let bad = |e: serde_json::Error| D::Error::custom(format!("{kind} step: {e}"));
        Ok(match kind.as_str() {
            "form" => Step::Form(serde_json::from_value(body).map_err(bad)?),
            "check" => Step::Check(serde_json::from_value(body).map_err(bad)?),
            "migration" => Step::Migration(serde_json::from_value(body).map_err(bad)?),
            "summary" => Step::Summary(serde_json::from_value(body).map_err(bad)?),
            _ => Step::Unknown(v),
        })
    }
}

impl Serialize for Step {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (kind, mut v) = match self {
            Step::Form(x) => ("form", serde_json::to_value(x)),
            Step::Check(x) => ("check", serde_json::to_value(x)),
            Step::Migration(x) => ("migration", serde_json::to_value(x)),
            Step::Summary(x) => ("summary", serde_json::to_value(x)),
            Step::Unknown(v) => return v.serialize(s),
        };
        if let Ok(o) = v.as_mut().map(|v| v.as_object_mut()) {
            if let Some(o) = o {
                o.insert("kind".into(), Value::String(kind.into()));
            }
        }
        v.map_err(serde::ser::Error::custom)?.serialize(s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WizardSpec {
    #[serde(default = "one")]
    pub wizard_version: u32,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub trigger: Trigger,
    pub steps: Vec<Step>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

// ------------------------------------------------------------------------------------------------ validation

fn plain(s: &str) -> bool {
    !s.chars().any(|c| c.is_control())
}

fn text_ok(s: &str, max: usize) -> bool {
    s.chars().count() <= max && plain(s)
}

fn label_ok(s: &str) -> bool {
    !s.trim().is_empty() && text_ok(s, MAX_LABEL)
}

/// Check a manifest's wizards against the limits. `declared` are the capability names the manifest (or the member) declared: a check may use only those.
/// A wizard of a newer `wizard_version` is checked for its id and size only. Unknown step kinds, field kinds, rules, checks and triggers pass (they are
/// refused at run time, [`WizardSpec::check_runnable`]).
pub fn validate_wizards(specs: &[WizardSpec], declared: &[&str]) -> Result<(), String> {
    if specs.len() > MAX_WIZARDS {
        return Err(format!("wizards: {} wizards, at most {MAX_WIZARDS}", specs.len()));
    }
    for (i, w) in specs.iter().enumerate() {
        if !id_ok(&w.id) {
            return Err(format!("wizards[{i}]: id is lowercase letters, digits and dashes"));
        }
        if specs[..i].iter().any(|o| o.id == w.id) {
            return Err(format!("wizard {:?}: the id is used twice", w.id));
        }
        validate_one(w, declared).map_err(|e| format!("wizard {:?}: {e}", w.id))?;
    }
    Ok(())
}

fn validate_one(w: &WizardSpec, declared: &[&str]) -> Result<(), String> {
    if w.wizard_version == 0 {
        return Err("wizard_version starts at 1".into());
    }
    if !text_ok(&w.title, MAX_LABEL) {
        return Err(format!("title is at most {MAX_LABEL} characters"));
    }
    if w.steps.is_empty() || w.steps.len() > MAX_STEPS {
        return Err(format!("1 to {MAX_STEPS} steps"));
    }
    let newer = w.wizard_version > WIZARD_VERSION;
    match (&w.trigger.kind(), &w.trigger.from) {
        (TriggerKind::OnUpdate, Some(r)) if !newer => {
            VersionRange::parse(r).map_err(|e| format!("trigger.from: {e}"))?;
        }
        (TriggerKind::OnUpdate, _) | (TriggerKind::Unknown(_), _) => {}
        (_, Some(_)) if !newer => return Err("trigger.from belongs to on_update only".into()),
        _ => {}
    }
    if w.trigger.on.is_empty() || w.trigger.on.len() > 32 {
        return Err("trigger.on is a short word".into());
    }
    let mut ids: Vec<String> = Vec::new();
    let mut field_ids: Vec<&str> = Vec::new();
    for (i, s) in w.steps.iter().enumerate() {
        let sid = s.id();
        if !id_ok(&sid) {
            return Err(format!("step {}: id is lowercase letters, digits and dashes", i + 1));
        }
        if ids.contains(&sid) {
            return Err(format!("step {sid:?}: the id is used twice"));
        }
        ids.push(sid.clone());
        if newer {
            continue;
        }
        let at = |e: String| format!("step {sid:?}: {e}");
        match s {
            Step::Form(f) => validate_form(f, &mut field_ids).map_err(at)?,
            Step::Check(c) => validate_check(c, declared).map_err(at)?,
            Step::Migration(m) => validate_migration(m).map_err(at)?,
            Step::Summary(x) => {
                if i + 1 != w.steps.len() {
                    return Err(at("a summary is the last step".into()));
                }
                if !text_ok(&x.text, MAX_TEXT) || !text_ok(&x.title, MAX_LABEL) {
                    return Err(at(format!("text is at most {MAX_TEXT} characters")));
                }
            }
            Step::Unknown(v) => {
                if v.to_string().len() > 4 * MAX_TEXT {
                    return Err(at("a step of a kind this IYAKKI does not know is at most 2400 characters".into()));
                }
            }
        }
    }
    Ok(())
}

fn validate_form<'a>(f: &'a FormStep, seen: &mut Vec<&'a str>) -> Result<(), String> {
    if !text_ok(&f.title, MAX_LABEL) {
        return Err(format!("title is at most {MAX_LABEL} characters"));
    }
    if f.fields.is_empty() || f.fields.len() > MAX_FIELDS {
        return Err(format!("a form has 1 to {MAX_FIELDS} fields"));
    }
    for fl in &f.fields {
        if !id_ok(&fl.id) {
            return Err(format!("field {:?}: id is lowercase letters, digits and dashes", fl.id));
        }
        if seen.contains(&fl.id.as_str()) {
            return Err(format!("field {:?}: the id is used twice in this wizard", fl.id));
        }
        seen.push(&fl.id);
        let at = |e: &str| format!("field {:?}: {e}", fl.id);
        if !label_ok(&fl.label) || !text_ok(&fl.help, MAX_TEXT) {
            return Err(at(&format!("label is 1 to {MAX_LABEL} characters, help at most {MAX_TEXT}")));
        }
        if fl.rules.len() > MAX_RULES {
            return Err(at(&format!("at most {MAX_RULES} rules")));
        }
        let kind = fl.kind();
        if fl.choices.len() > MAX_CHOICES {
            return Err(at(&format!("at most {MAX_CHOICES} choices")));
        }
        for c in &fl.choices {
            if c.value.is_empty() || !text_ok(&c.value, MAX_DEFAULT) || !label_ok(&c.label) {
                return Err(at("each choice has a value and a label"));
            }
        }
        for (i, c) in fl.choices.iter().enumerate() {
            if fl.choices[..i].iter().any(|o| o.value == c.value) {
                return Err(at("two choices have the same value"));
            }
        }
        match (&kind, &fl.default) {
            (FieldKind::Secret, Some(_)) => return Err(at("a secret has no default (secrets go to the vault, not into a manifest)")),
            (FieldKind::Toggle, Some(d)) if !d.is_boolean() => return Err(at("a toggle's default is true or false")),
            (FieldKind::Text | FieldKind::Path | FieldKind::Choice, Some(d)) => {
                let Some(t) = d.as_str() else { return Err(at("the default is text")) };
                if !text_ok(t, MAX_DEFAULT) {
                    return Err(at(&format!("the default is at most {MAX_DEFAULT} characters")));
                }
                if kind == FieldKind::Path && (!t.starts_with('/') || t.split('/').any(|s| s == "..")) {
                    return Err(at("a path default is absolute without .."));
                }
                if kind == FieldKind::Choice && !fl.choices.iter().any(|c| c.value == t) {
                    return Err(at("the default is not one of the choices"));
                }
            }
            _ => {}
        }
        if kind == FieldKind::Choice && fl.choices.is_empty() {
            return Err(at("a choice field lists its choices"));
        }
        for r in &fl.rules {
            validate_rule(r).map_err(|e| at(&e))?;
        }
    }
    Ok(())
}

fn validate_rule(r: &Rule) -> Result<(), String> {
    if !text_ok(&r.message, MAX_LABEL) {
        return Err(format!("rule {:?}: the message is at most {MAX_LABEL} characters", r.rule));
    }
    match r.rule.as_str() {
        "min_len" | "max_len" => {
            if !r.value.as_ref().and_then(Value::as_u64).is_some_and(|n| n <= MAX_RULE_NUMBER) {
                return Err(format!("rule {:?} takes a number up to {MAX_RULE_NUMBER}", r.rule));
            }
        }
        "one_of" => {
            let ok = r.value.as_ref().and_then(Value::as_array).is_some_and(|a| !a.is_empty() && a.len() <= MAX_CHOICES && a.iter().all(|x| x.as_str().is_some_and(|s| text_ok(s, MAX_DEFAULT))));
            if !ok {
                return Err(format!("rule \"one_of\" takes a list of 1 to {MAX_CHOICES} short texts"));
            }
        }
        "no_whitespace" | "absolute_path" => {}
        _ => {}
    }
    Ok(())
}

fn validate_check(c: &CheckStep, declared: &[&str]) -> Result<(), String> {
    if !text_ok(&c.title, MAX_LABEL) {
        return Err(format!("title is at most {MAX_LABEL} characters"));
    }
    let need = match c.check.as_str() {
        "net.reach" => Some("net.connect"),
        "path.readable" => Some("files.read"),
        "path.writable" => Some("files.write"),
        "capability.held" => None,
        _ => return Ok(()), // a check this IYAKKI does not know: refused at run time
    };
    let Some(p) = c.param.as_deref().filter(|p| !p.is_empty()) else { return Err(format!("check {:?} needs a param", c.check)) };
    if c.check == "capability.held" {
        if appcaps::lookup(p).is_none() {
            return Err(format!("check \"capability.held\": {p:?} is not a capability of this IYAKKI"));
        }
        return Ok(());
    }
    let need = need.unwrap_or_default();
    if !declared.contains(&need) {
        return Err(format!("check {:?} uses {need}, which the manifest does not declare in its capabilities", c.check));
    }
    appcaps::validate_requests(&[CapabilityRequest { name: need.into(), why: "wizard check".into(), param: Some(p.to_string()), extra: Default::default() }]).map_err(|e| format!("check {:?}: {e}", c.check))
}

fn validate_migration(m: &MigrationStep) -> Result<(), String> {
    if !text_ok(&m.title, MAX_LABEL) {
        return Err(format!("title is at most {MAX_LABEL} characters"));
    }
    if m.migration.len() > MAX_REF || !rel_path_ok(&m.migration) {
        return Err("migration is the name or file of a migration inside the app (no .., no leading /)".into());
    }
    for (label, v) in [("from", &m.from), ("to", &m.to)] {
        if let Some(v) = v {
            if parse_version(v).is_none() {
                return Err(format!("{label} is a version like 1.2.0"));
            }
        }
    }
    Ok(())
}

impl WizardSpec {
    /// Whether the engine of this IYAKKI may run the wizard. This is where unknown things are refused (with the words for the card), not at parse time.
    pub fn check_runnable(&self) -> Result<(), String> {
        if self.wizard_version > WIZARD_VERSION {
            return Err(format!("Needs a newer IYAKKI: this wizard is version {}, this IYAKKI runs up to version {WIZARD_VERSION}.", self.wizard_version));
        }
        if let TriggerKind::Unknown(t) = self.trigger.kind() {
            return Err(format!("This IYAKKI does not know the trigger {t:?}."));
        }
        for s in &self.steps {
            match s {
                Step::Unknown(_) => return Err(format!("This IYAKKI does not know the step kind {:?}; update IYAKKI to run this wizard.", s.kind_name())),
                Step::Form(f) => {
                    for fl in &f.fields {
                        if let FieldKind::Unknown(k) = fl.kind() {
                            return Err(format!("This IYAKKI does not know the field kind {k:?} (field {:?}); update IYAKKI to run this wizard.", fl.id));
                        }
                        if let Some(r) = fl.rules.iter().find(|r| !matches!(r.rule.as_str(), "min_len" | "max_len" | "no_whitespace" | "absolute_path" | "one_of")) {
                            return Err(format!("This IYAKKI does not know the rule {:?} (field {:?}); update IYAKKI to run this wizard.", r.rule, fl.id));
                        }
                    }
                }
                Step::Check(c) if !matches!(c.check.as_str(), "net.reach" | "path.readable" | "path.writable" | "capability.held") => {
                    return Err(format!("This IYAKKI does not know the check {:?}; update IYAKKI to run this wizard.", c.check));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
