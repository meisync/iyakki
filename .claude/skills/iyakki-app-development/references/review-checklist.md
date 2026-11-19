# Review checklist (answer yes or no to each; one "no" means send it back with the number)

1. Cargo.toml [dependencies] holds `iyakki-ui-sdk` and no other IYAKKI crate except `iyakki-sdk-tile`.
2. No `std::fs`, `std::net`, `std::process`, `iyakki_agent`, `iyakki_shell`, `iyakki_uikit`, `/proc`, `/sys`, `/dev` in `src/`.
3. No colour written in the source (`rgb(`, `[r, g, b]`).
4. No number of 3.0 or more in drawing code.
5. Every control has a name equal to its visible label.
6. Every target is at least `m.min_target()`.
7. Layout starts from `c.size()` / `cx.size_class()`; Compact has one column.
8. Tests cover three sizes and text 3x.
9. `app.toml` id equals `ID`; `ports` lists exactly the ports used.
10. `view` does not change state; `Tick` polls ports.
11. A port refusal is shown in words.
12. Every changing tool has `ask = true`.
13. Every on-screen switch/setting/action has a chat tool that shares its code.
14. No invented approval wording, consent or privacy rule; nothing secret in a tool argument, log or test.
15. Embedded views have separate action bases; Esc is offered to the view first.
16. Golden files are committed and were read.
17. `./run.sh app-check` prints no problems; the report says what was not run.
18. Rows in `FEATURES.md` are honest (Impl = code written, Test = what was run).
