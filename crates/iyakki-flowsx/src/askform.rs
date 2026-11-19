//! `ask_form`: the assistant asks the person to fill in missing input as a small form. The spec is validated here (bounded counts and
//! lengths), the chat draws it, and the filled values go back as the next message. Secret fields are never part of that message:
//! the shell stores them in the person's vault and only the vault name travels. Nothing here sees a secret value.

use iyakki_flow::Param;
use serde_json::{json, Value};

pub const MAX_FIELDS: usize = 6;
pub const MAX_TITLE: usize = 100;
pub const MAX_LABEL: usize = 80;
pub const MAX_HELP: usize = 200;
pub const MAX_CHOICES: usize = 12;
pub const MAX_CHOICE_LEN: usize = 60;
pub const MAX_VALUE: usize = 2000;
pub const MAX_NAME: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Number,
    Choice,
    YesNo,
    Secret,
    Path,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "text" | "" => Kind::Text,
            "number" => Kind::Number,
            "choice" => Kind::Choice,
            "yes-no" | "yesno" | "yes_no" | "bool" | "boolean" => Kind::YesNo,
            "secret" => Kind::Secret,
            "path" => Kind::Path,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub kind: Kind,
    pub required: bool,
    pub default: Option<String>,
    pub choices: Vec<String>,
    pub help: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormSpec {
    pub title: String,
    pub fields: Vec<Field>,
}

fn clip(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= MAX_NAME && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The tool call's arguments as a form: `{title, fields:[{name,label?,kind,required?,default?,choices?,help?}]}`.
pub fn parse(args: &Value) -> Result<FormSpec, String> {
    let title = clip(args.get("title").and_then(Value::as_str).unwrap_or(""), MAX_TITLE);
    let raw = args.get("fields").and_then(Value::as_array).ok_or("ask_form needs `fields` (a list of 1 to 6 fields)")?;
    if raw.is_empty() || raw.len() > MAX_FIELDS {
        return Err(format!("ask_form takes 1 to {MAX_FIELDS} fields, got {}", raw.len()));
    }
    let mut fields: Vec<Field> = Vec::new();
    for f in raw {
        let name = f.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if !valid_name(&name) {
            return Err(format!("a field name is letters, digits, _ and - (up to {MAX_NAME}); got `{}`", clip(&name, 50)));
        }
        if fields.iter().any(|x| x.name == name) {
            return Err(format!("field `{name}` appears twice"));
        }
        let ks = f.get("kind").and_then(Value::as_str).unwrap_or("text");
        let kind = Kind::parse(ks).ok_or_else(|| format!("field `{name}`: kind `{}` is not text, number, choice, yes-no, secret or path", clip(ks, 20)))?;
        let label = clip(f.get("label").and_then(Value::as_str).unwrap_or(""), MAX_LABEL);
        let label = if label.is_empty() { name.clone() } else { label };
        let required = f.get("required").and_then(Value::as_bool).unwrap_or(false);
        let help = clip(f.get("help").and_then(Value::as_str).unwrap_or(""), MAX_HELP);
        let mut choices = Vec::new();
        if kind == Kind::Choice {
            for c in f.get("choices").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                let c = clip(c, MAX_CHOICE_LEN);
                if !c.is_empty() && !choices.contains(&c) {
                    choices.push(c);
                }
            }
            if choices.len() < 2 || choices.len() > MAX_CHOICES {
                return Err(format!("field `{name}`: a choice needs 2 to {MAX_CHOICES} `choices`"));
            }
        }
        let default = match f.get("default") {
            None | Some(Value::Null) => None,
            Some(_) if kind == Kind::Secret => return Err(format!("field `{name}`: a secret has no default")),
            Some(Value::String(s)) => Some(clip(s, MAX_VALUE)),
            Some(Value::Bool(b)) => Some(if *b { "yes" } else { "no" }.to_string()),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(_) => return Err(format!("field `{name}`: the default is a string, number or yes/no")),
        };
        let fld = Field { name: name.clone(), label, kind, required, default, choices, help };
        if let Some(d) = &fld.default {
            if !d.is_empty() {
                check_value(&fld, d).map_err(|e| format!("field `{name}`: default: {e}"))?;
            }
        }
        fields.push(fld);
    }
    Ok(FormSpec { title: if title.is_empty() { "A few details".into() } else { title }, fields })
}

/// One value against its field (empty is allowed here; `required` is checked by [`FormSpec::check`]).
fn check_value(f: &Field, v: &str) -> Result<(), String> {
    let max = if f.kind == Kind::Secret { 4096 } else { MAX_VALUE };
    if v.chars().count() > max {
        return Err(format!("too long (at most {max} characters)"));
    }
    if v.contains('\0') {
        return Err("contains a character that is not allowed".into());
    }
    if v.is_empty() {
        return Ok(());
    }
    match f.kind {
        Kind::Number if v.trim().parse::<f64>().ok().filter(|n| n.is_finite()).is_none() => Err("enter a number".into()),
        Kind::Choice if !f.choices.iter().any(|c| c == v) => Err("pick one of the choices".into()),
        Kind::YesNo if !matches!(v, "yes" | "no") => Err("answer yes or no".into()),
        _ => Ok(()),
    }
}

impl FormSpec {
    /// Check filled values (one per field, in order). `Err((field index, message))` names the first problem.
    pub fn check(&self, values: &[String]) -> Result<(), (usize, String)> {
        for (i, f) in self.fields.iter().enumerate() {
            let v = values.get(i).map(|s| s.trim()).unwrap_or("");
            if v.is_empty() && f.required {
                return Err((i, format!("{} is required", f.label)));
            }
            check_value(f, v).map_err(|e| (i, format!("{}: {e}", f.label)))?;
        }
        Ok(())
    }

    /// What the chat shows in the conversation when the form opens: the title and the labels, no values.
    pub fn shown_text(&self) -> String {
        let l: Vec<String> = self.fields.iter().map(|f| format!("{}{}", f.label, if f.required { " *" } else { "" })).collect();
        format!("{}\n\nPlease fill in the form: {}", self.title, l.join(", "))
    }

    /// The submitted values as the message that goes back to the assistant. A secret is listed by the vault name it was stored under,
    /// never by value; `secrets[i]` is that name for field `i`.
    pub fn answer_text(&self, values: &[String], secrets: &[Option<String>]) -> String {
        let mut out = format!("Form \"{}\" filled in:", self.title);
        for (i, f) in self.fields.iter().enumerate() {
            let v = values.get(i).map(|s| s.trim()).unwrap_or("");
            match (f.kind, secrets.get(i).cloned().flatten()) {
                (Kind::Secret, Some(n)) => out.push_str(&format!("\n- {}: stored in the vault as {{{{secret.{n}}}}} (the value is not shown)", f.name)),
                (Kind::Secret, None) => out.push_str(&format!("\n- {}: left empty", f.name)),
                _ if v.is_empty() => out.push_str(&format!("\n- {}: (empty)", f.name)),
                _ => out.push_str(&format!("\n- {}: {}", f.name, v)),
            }
        }
        out
    }

    /// The vault name a secret field is stored under.
    pub fn secret_name(&self, i: usize) -> String {
        let n: String = self.fields[i].name.to_ascii_lowercase().chars().map(|c| if c == '_' { '-' } else { c }).collect();
        format!("flow.{n}")
    }
}

pub const CANCELLED: &str = "Form cancelled.";

/// A flow's declared inputs as `ask_form` fields: the required ones without a default that `given` does not have yet.
pub fn fields_for_missing(inputs: &[Param], given: &Value) -> Vec<Value> {
    inputs
        .iter()
        .filter(|p| p.required && p.default.is_none() && given.get(&p.name).is_none_or(|v| v.is_null() || v.as_str() == Some("")))
        .take(MAX_FIELDS)
        .map(|p| {
            let kind = match p.ty.as_str() {
                "secret" => "secret",
                "number" | "integer" | "int" | "float" => "number",
                "bool" | "boolean" => "yes-no",
                "path" | "file" | "dir" => "path",
                _ => "text",
            };
            json!({"name": p.name, "label": p.name, "kind": kind, "required": true, "help": p.description})
        })
        .collect()
}

pub fn def_description() -> &'static str {
    "Ask the person to fill in missing input as a form in the chat (use it when a flow, or a step you build, lacks a required value; flow_run tells you which). `title`, and `fields`: 1 to 6 of {name, label, kind: text|number|choice|yes-no|secret|path, required, default, choices (for choice, 2 to 12), help}. A secret goes straight into the person's vault and you only learn its name (use {{secret.NAME}}); never ask for one any other way. After this call, stop: the filled values arrive as the next message, or \"Form cancelled.\""
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> FormSpec {
        parse(&json!({"title": "Deploy", "fields": [
            {"name": "host", "label": "Host", "kind": "text", "required": true},
            {"name": "port", "kind": "number", "default": 22},
            {"name": "env", "kind": "choice", "choices": ["dev", "prod"], "required": true},
            {"name": "dry", "kind": "yes-no", "default": false},
            {"name": "token", "kind": "secret", "required": true}
        ]}))
        .unwrap()
    }

    #[test]
    fn a_good_spec_parses_and_defaults_are_text() {
        let s = spec();
        assert_eq!(s.fields.len(), 5);
        assert_eq!(s.fields[1].default.as_deref(), Some("22"));
        assert_eq!(s.fields[3].default.as_deref(), Some("no"));
        assert_eq!(s.fields[1].label, "port");
    }

    #[test]
    fn counts_lengths_kinds_names_and_secret_defaults_are_bounded() {
        let many: Vec<Value> = (0..7).map(|i| json!({"name": format!("f{i}")})).collect();
        assert!(parse(&json!({"fields": many})).is_err());
        assert!(parse(&json!({"fields": []})).is_err());
        assert!(parse(&json!({})).is_err());
        assert!(parse(&json!({"fields": [{"name": "bad name"}]})).is_err());
        assert!(parse(&json!({"fields": [{"name": "a"}, {"name": "a"}]})).is_err());
        assert!(parse(&json!({"fields": [{"name": "a", "kind": "date"}]})).is_err());
        assert!(parse(&json!({"fields": [{"name": "a", "kind": "choice", "choices": ["x"]}]})).is_err());
        assert!(parse(&json!({"fields": [{"name": "a", "kind": "secret", "default": "hunter2"}]})).is_err());
        assert!(parse(&json!({"fields": [{"name": "a", "kind": "number", "default": "abc"}]})).is_err());
        let long = "x".repeat(500);
        let s = parse(&json!({"title": long, "fields": [{"name": "a", "label": long, "help": long}]})).unwrap();
        assert!(s.title.chars().count() <= MAX_TITLE && s.fields[0].label.chars().count() <= MAX_LABEL && s.fields[0].help.chars().count() <= MAX_HELP);
    }

    #[test]
    fn values_are_checked_required_number_choice_and_yes_no() {
        let s = spec();
        let ok: Vec<String> = ["h", "22", "dev", "no", "tok"].iter().map(|x| x.to_string()).collect();
        assert!(s.check(&ok).is_ok());
        let mut v = ok.clone();
        v[0] = "  ".into();
        assert_eq!(s.check(&v).unwrap_err().0, 0);
        v = ok.clone();
        v[1] = "twenty".into();
        assert_eq!(s.check(&v).unwrap_err().0, 1);
        v = ok.clone();
        v[2] = "stage".into();
        assert_eq!(s.check(&v).unwrap_err().0, 2);
        v = ok.clone();
        v[3] = "maybe".into();
        assert_eq!(s.check(&v).unwrap_err().0, 3);
        v = ok.clone();
        v[4] = String::new();
        assert!(s.check(&v).unwrap_err().1.contains("required"));
        v = ok;
        v[0] = "a\0b".into();
        assert!(s.check(&v).is_err());
    }

    #[test]
    fn the_answer_never_carries_a_secret_value() {
        let s = spec();
        let vals: Vec<String> = ["h", "22", "dev", "no", "SUPER-SECRET-VALUE"].iter().map(|x| x.to_string()).collect();
        let t = s.answer_text(&vals, &[None, None, None, None, Some(s.secret_name(4))]);
        assert!(!t.contains("SUPER-SECRET-VALUE"));
        assert!(t.contains("{{secret.flow.token}}") && t.contains("- host: h"));
        assert!(!s.shown_text().contains("SUPER"));
    }

    #[test]
    fn missing_flow_inputs_become_fields() {
        let ps = vec![
            Param { name: "target".into(), ty: "string".into(), required: true, default: None, description: "where".into() },
            Param { name: "key".into(), ty: "secret".into(), required: true, default: None, description: String::new() },
            Param { name: "n".into(), ty: "number".into(), required: true, default: Some(json!(1)), description: String::new() },
            Param { name: "have".into(), ty: "string".into(), required: true, default: None, description: String::new() },
        ];
        let f = fields_for_missing(&ps, &json!({"have": "x"}));
        assert_eq!(f.len(), 2);
        assert_eq!(f[1]["kind"], "secret");
        assert!(parse(&json!({"fields": f})).is_ok());
    }
}
