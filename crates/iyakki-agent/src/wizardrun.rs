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
pub use iyakki_shared::wizardrun::{FieldView, State, View};

pub use iyakki_shared::wizardrun::{Env, FieldError, Run};

#[cfg(test)]
#[path = "wizardrun_tests.rs"]
mod tests;
