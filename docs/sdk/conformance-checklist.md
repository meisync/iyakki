# Conformance checklist for app reviewers

Go line by line. An app is accepted when every line is a yes (or "not applicable", with the reason in the review). Each line names how to check it.

## Dependencies and source
1. `Cargo.toml` `[dependencies]` names `iyakki-ui-sdk` (and `iyakki-sdk-tile` for a tile) and no other IYAKKI crate. (generated test `the_source_keeps_the_sdk_rules`)
2. The source has no `std::fs`, `std::net`, `std::process`, `iyakki_uikit`, `iyakki_agent`, `iyakki_shell`, no `/proc`, `/sys`, `/dev`. (same test)
3. Dependencies besides IYAKKI crates are permissively licensed (`cargo deny`), and there are as few as possible.
4. No personal names, emails, home paths, IPs or secrets in code, tests or fixtures.

## Drawing
5. No colour literal; colours from `cx.colors()` (`rgb(` scan).
6. No styling number: sizes, gaps, radii, lines from `m.size`, `m.gap`, `m.radius`, `m.line` (`no_styling_numbers_in_the_drawing_code`).
7. Layout from `canvas.size()` and `cx.size_class()`; compact is one column.
8. Every control has a name (the screen reader's name equals the visible label) and a role. (controls test)
9. Every target is at least `m.min_target()`; nothing leaves the pane at 360x740, 800x480, 1280x720 and at text 3x. (controls test)
10. Long text is shortened with `canvas::fit` or wrapped; nothing overflows at 3x.
11. Contrast: only theme colours; an own accent goes through `cx.accent`.
12. The app looks right in each shipped skin (switch with `Host::set_look` in a test).
13. Fonts by role only.

## Behaviour
14. Keyboard: every action works without a pointer; Esc goes back (after the embedded view had it); Enter acts on the focused control.
15. `wants_keyboard` is true while a text field has focus.
16. A port refusal is shown to the person in words.
17. State is in the struct; `view` does not change it and is cheap.
18. Persistence uses `StoragePort`; a failed save is shown.

## Permissions and chat
19. `app.toml` `ports` lists exactly what the app uses.
20. Every tool that changes data or the machine has `ask = true`; reads are free.
21. Every switch, setting and action in a screen has a chat tool with the same code path.
22. Nothing secret is typed through chat or a tool argument.
23. No consent, wording of an approval card or privacy rule is invented by the app: those come from the core. (escalate if unsure)

## Tests and rows
24. Golden files are committed and were read.
25. The tests above pass at three sizes and text 3x; the app has tests for its own logic and for each tool.
26. `cargo test -p iyakki-core --test boundary --test dependency_ratchet --test ui_agent_uses --test feature_matrix` pass.
27. `FEATURES.md` has rows with honest Impl and Test (Impl = code written, Test = what was run); the docs for the app are updated.
28. `python3 dist/sdk/gen-widget-docs.py --check` passes.
