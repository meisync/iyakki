//! The feature board's model (alpha.7 S2: the BOARD tile becomes the crate `iyakki-tile-board`). `iyakki-shared` re-exports every
//! module under its old path (`iyakki_shared::featureboard`, `::boarddeps`, `::assign`, `::testresults`), so no other crate changes.
//! Rule (crates/iyakki-core/tests/dependency_ratchet.rs): depends on `iyakki-clock` only, never on the agent, the shell or a tile.

pub mod assign;
pub mod boardtrack;
pub mod coverage;
pub mod boarddeps;
pub mod featureboard;
pub mod testresults;

/// Split `cargo test -p "my crate"` into words (single and double quotes group; no expansion of any kind).
pub fn split_command(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut q, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for c in s.chars() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                q = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What to do with rows that become ready when an agent finishes one: nothing, offer them, or start tasks for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AutoAssign {
    Off,
    Ask,
    On,
}

impl AutoAssign {
    pub fn key(self) -> &'static str {
        match self {
            AutoAssign::Off => "off",
            AutoAssign::Ask => "ask",
            AutoAssign::On => "on",
        }
    }
    pub fn parse(s: &str) -> AutoAssign {
        match s.trim().to_lowercase().as_str() {
            "off" => AutoAssign::Off,
            "on" => AutoAssign::On,
            _ => AutoAssign::Ask,
        }
    }
    pub fn next(self) -> AutoAssign {
        match self {
            AutoAssign::Off => AutoAssign::Ask,
            AutoAssign::Ask => AutoAssign::On,
            AutoAssign::On => AutoAssign::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{split_command, AutoAssign};

    #[test]
    fn split_command_groups_quotes_and_does_no_expansion() {
        assert_eq!(split_command("cargo test -p \"my crate\" '' $HOME"), vec!["cargo", "test", "-p", "my crate", "", "$HOME"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn auto_assign_cycles_and_parses() {
        assert_eq!(AutoAssign::Off.next(), AutoAssign::Ask);
        assert_eq!(AutoAssign::Ask.next(), AutoAssign::On);
        assert_eq!(AutoAssign::On.next(), AutoAssign::Off);
        assert_eq!(AutoAssign::parse(" ON "), AutoAssign::On);
        assert_eq!(AutoAssign::parse("garbage"), AutoAssign::Ask);
        assert_eq!(AutoAssign::parse(AutoAssign::Off.key()), AutoAssign::Off);
    }
}
