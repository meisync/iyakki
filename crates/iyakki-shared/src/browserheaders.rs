//! Types moved out of `iyakki_agent::browserheaders` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeaderRule {
    pub id: String,
    /// A label for the list ("Staging token").
    pub name: String,
    /// The domain filter: entries separated by commas (`api.example.org`, `*.corp.example`); empty when only a prefix is given.
    pub domains: String,
    /// An optional URL prefix (`https://example.org/api`); the rule applies only below it.
    pub prefix: String,
    /// An optional regular expression the request's path (and query, `/api/v2/items?id=7`) must match, on top of the domain filter and
    /// the prefix: `^/api/v[0-9]+/`, `\\.json$`. Narrows a rule; never sends a header to a site by itself. Matched by the small in-house
    /// engine (a step limit stops a pattern that would run forever).
    #[serde(default)]
    pub path_regex: String,
    pub header: String,
    pub enabled: bool,
    /// The person ticked "I know" for Cookie or Authorization.
    pub ack: bool,
}
