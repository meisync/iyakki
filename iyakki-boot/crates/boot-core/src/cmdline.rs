// SPDX-License-Identifier: MIT OR Apache-2.0
//! Kernel command line rules: validate, append, override and remove parameters.
//!
//! A line is parameters separated by single spaces. Quotes and backslashes are not allowed, so splitting on spaces is always exact.
//! This is deliberate: quoting and escaping rules differ between boot loaders, the kernel and init systems, so a line with them
//! would mean different things on different systems. Forbidding them keeps one line portable and the split exact.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Longest command line accepted (the x86 kernel limit is 2048 including the final NUL).
pub const MAX_LEN: usize = 2047;

/// Why a command line or an edit was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CmdError {
    /// The result would be longer than the limit.
    TooLong,
    /// A character that is never allowed (control, non-ASCII, quote, backslash).
    Forbidden(char),
    /// An edit with an empty parameter name.
    EmptyKey,
    /// Editing is switched off because Secure Boot is on.
    Locked,
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CmdError::TooLong => write!(f, "command line longer than {MAX_LEN} bytes"),
            CmdError::Forbidden(c) => write!(f, "character {:?} is not allowed in a command line", c),
            CmdError::EmptyKey => write!(f, "empty parameter name"),
            CmdError::Locked => write!(f, "command line edits are refused while Secure Boot is on"),
        }
    }
}

/// One change to a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Add a parameter at the end (an identical one is not added twice).
    Append(String),
    /// Replace every parameter with the same name (text before `=`), or add it.
    Override(String),
    /// Drop every parameter with this name, or exactly this `name=value` when a value is given.
    Remove(String),
}

impl Edit {
    /// Parse user text: `+x` append, `-x` remove, `x=y` override, bare `x` append.
    pub fn parse(text: &str) -> Edit {
        let t = text.trim();
        if let Some(r) = t.strip_prefix('+') {
            Edit::Append(r.to_string())
        } else if let Some(r) = t.strip_prefix('-') {
            Edit::Remove(r.to_string())
        } else if t.contains('=') {
            Edit::Override(t.to_string())
        } else {
            Edit::Append(t.to_string())
        }
    }
}

/// Policy for edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// Secure Boot is on right now.
    pub secure_boot: bool,
    /// Refuse every edit while Secure Boot is on.
    pub refuse_edit_on_secure_boot: bool,
    /// Length limit.
    pub max_len: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { secure_boot: false, refuse_edit_on_secure_boot: true, max_len: MAX_LEN }
    }
}

/// Check one piece of text (a whole line or one parameter).
pub fn validate(s: &str) -> Result<(), CmdError> {
    for c in s.chars() {
        if !(' '..='~').contains(&c) || c == '"' || c == '\'' || c == '\\' {
            return Err(CmdError::Forbidden(c));
        }
    }
    if s.len() > MAX_LEN {
        return Err(CmdError::TooLong);
    }
    Ok(())
}

fn key_of(p: &str) -> &str {
    p.split('=').next().unwrap_or(p)
}

/// Split a line into parameters (runs of spaces collapse).
pub fn params(line: &str) -> Vec<&str> {
    line.split(' ').filter(|p| !p.is_empty()).collect()
}

/// Apply edits to a base line. The base itself must already be valid.
pub fn apply(base: &str, edits: &[Edit], rules: &Rules) -> Result<String, CmdError> {
    if !edits.is_empty() && rules.secure_boot && rules.refuse_edit_on_secure_boot {
        return Err(CmdError::Locked);
    }
    validate(base)?;
    let mut list: Vec<String> = params(base).into_iter().map(|p| p.to_string()).collect();
    for e in edits {
        match e {
            Edit::Append(t) | Edit::Override(t) | Edit::Remove(t) => {
                validate(t)?;
                if t.trim().is_empty() || key_of(t.trim()).is_empty() {
                    return Err(CmdError::EmptyKey);
                }
                if t.contains(' ') {
                    // One edit is one parameter; several need several edits.
                    return Err(CmdError::Forbidden(' '));
                }
            }
        }
        match e {
            Edit::Append(t) => {
                if !list.iter().any(|p| p == t) {
                    list.push(t.clone());
                }
            }
            Edit::Override(t) => {
                let k = key_of(t);
                let pos = list.iter().position(|p| key_of(p) == k);
                list.retain(|p| key_of(p) != k);
                let at = pos.map_or(list.len(), |p| p.min(list.len()));
                list.insert(at, t.clone());
            }
            Edit::Remove(t) => {
                if t.contains('=') {
                    list.retain(|p| p != t);
                } else {
                    list.retain(|p| key_of(p) != t.as_str());
                }
            }
        }
    }
    let out = list.join(" ");
    if out.len() > rules.max_len.min(MAX_LEN) {
        return Err(CmdError::TooLong);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> Rules {
        Rules::default()
    }

    #[test]
    fn append_override_remove() {
        let out = apply(
            "quiet root=/dev/sda1 loglevel=3",
            &[Edit::Append("nomodeset".into()), Edit::Override("loglevel=7".into()), Edit::Remove("quiet".into())],
            &r(),
        )
        .unwrap();
        assert_eq!(out, "root=/dev/sda1 loglevel=7 nomodeset");
    }

    #[test]
    fn override_keeps_position_and_collapses_duplicates() {
        let out = apply("a=1 b=2 a=3", &[Edit::Override("a=9".into())], &r()).unwrap();
        assert_eq!(out, "a=9 b=2");
    }

    #[test]
    fn append_is_idempotent_and_remove_exact_value() {
        let out = apply("x=1 x=2", &[Edit::Append("x=1".into()), Edit::Remove("x=1".into())], &r()).unwrap();
        assert_eq!(out, "x=2");
    }

    #[test]
    fn forbidden_characters() {
        assert_eq!(apply("a", &[Edit::Append("b\"c".into())], &r()), Err(CmdError::Forbidden('"')));
        assert_eq!(apply("a\nb", &[], &r()), Err(CmdError::Forbidden('\n')));
        assert!(matches!(apply("é", &[], &r()), Err(CmdError::Forbidden(_))));
        assert_eq!(apply("a", &[Edit::Append("x y".into())], &r()), Err(CmdError::Forbidden(' ')));
        assert_eq!(apply("a", &[Edit::Append("=v".into())], &r()), Err(CmdError::EmptyKey));
        assert_eq!(apply("a", &[Edit::Append("".into())], &r()), Err(CmdError::EmptyKey));
    }

    #[test]
    fn length_limit() {
        let long = "a".repeat(MAX_LEN);
        assert_eq!(apply(&long, &[], &r()).unwrap().len(), MAX_LEN);
        assert_eq!(apply(&long, &[Edit::Append("b".into())], &r()), Err(CmdError::TooLong));
        let tiny = Rules { max_len: 5, ..r() };
        assert_eq!(apply("abc", &[Edit::Append("def".into())], &tiny), Err(CmdError::TooLong));
    }

    #[test]
    fn secure_boot_refuses_edits_only() {
        let sb = Rules { secure_boot: true, ..r() };
        assert_eq!(apply("a", &[Edit::Append("b".into())], &sb), Err(CmdError::Locked));
        assert_eq!(apply("a", &[], &sb).unwrap(), "a");
        let open = Rules { secure_boot: true, refuse_edit_on_secure_boot: false, ..r() };
        assert_eq!(apply("a", &[Edit::Append("b".into())], &open).unwrap(), "a b");
        let off = Rules { secure_boot: false, ..r() };
        assert!(apply("a", &[Edit::Append("b".into())], &off).is_ok());
    }

    #[test]
    fn parse_user_text() {
        assert_eq!(Edit::parse("+x"), Edit::Append("x".into()));
        assert_eq!(Edit::parse("-x"), Edit::Remove("x".into()));
        assert_eq!(Edit::parse("x=1"), Edit::Override("x=1".into()));
        assert_eq!(Edit::parse(" x "), Edit::Append("x".into()));
    }

    #[test]
    fn spaces_collapse() {
        assert_eq!(apply("a   b", &[Edit::Remove("zz".into())], &r()).unwrap(), "a b");
    }
}
