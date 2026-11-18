//! Types moved out of `iyakki_agent::buildtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSHARED BUILDS: builds_status shows the newest build per machine type in the team's build store (read-only, from the store's signed list). builds_store_set remembers the store address, the update public key and the read-token file (it asks; the token itself is never shown or stored by you). builds_fetch downloads the newest build of one machine type into a folder after the person approves the card; it installs nothing: tell the person the printed `iyakki dev update` line is theirs to run, or, when they want it installed on THIS device, use builds_install (asks every time; picks the newest build whose base layers this device holds). The store keeps only the latest build per machine type.";
