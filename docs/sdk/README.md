# Build an IYAKKI app: the developer guide

For a person who has the repository and has never seen it. An IYAKKI app is a small Rust program written against one crate, `iyakki-ui-sdk`. The same source draws on the device, in a desktop window, in a browser tab and as a WASM tile, and it reaches the machine only through ports that the core hands it. These pages teach that in the order you need it.

Internals, plans and the reasons behind the design stay in `docs/development/` (start with `UI-SDK.md` and `UI-SDK-DESIGN.md`). This folder is the way in.

| Read | When |
|---|---|
| [first-app.md](first-app.md) | Your first app in 15 minutes: make it, test it, see it, put it on a device |
| [concepts.md](concepts.md) | The core contract, ports, facets, approvals, audit, `UiCx`, `Host`, the display list, action numbers, tick, key, draw |
| [recipes/README.md](recipes/README.md) | Short examples that compile (and are tested): a pane, tabs, a list, a form, persistence, themed drawing, an approval card |
| [events.md](events.md) | Hear what happens on the machine (network, power, files, apps, ...), online and offline, and act on it |
| [embedding.md](embedding.md) | Put the assistant, a browser, the files, a terminal, an editor, a player or a camera in your layout |
| [dry-run.md](dry-run.md) | A worked app (counter, notes with persistence, embedded file browser) and what building it taught us |
| [reference/README.md](reference/README.md) | Every public type and call of the SDK, generated from the code |
| [themes-skins-fonts.md](themes-skins-fonts.md) | Tokens, skins, font roles, contrast, the three size classes and text at 3x, the style-literal lint |
| [permissions-and-privacy.md](permissions-and-privacy.md) | What the core refuses, approval cards, the audit log, and why every feature has a chat tool |
| [testing.md](testing.md) | Fakes, host tests, golden files, real-adapter tests, the contract tests, the fast loop |
| [packaging-and-publishing.md](packaging-and-publishing.md) | Tile packages, signing, the store: what exists today and what does not |
| [app-check](testing.md#app-check-the-fast-author-check) | `./run.sh app-check PATH`: only the checks an author can hit, a numbered fix list in seconds |
| [troubleshooting.md](troubleshooting.md) | Failures you will meet, with the fix |
| [conformance-checklist.md](conformance-checklist.md) | The list a reviewer uses before an app is accepted |

## Keeping these pages true

- `reference/*.md` are generated from the code by `python3 dist/sdk/gen-widget-docs.py`. Do not edit them: change a doc comment in `crates/iyakki-ui-sdk/src` and run the script.
- Every code block between `<!-- snippet: NAME.rs -->` markers is the file `recipes/NAME.rs`, which `crates/iyakki-ui-sdk/tests/docs_recipes.rs` compiles and runs. The script refreshes the blocks.
- The test `crates/iyakki-ui-sdk/tests/docs_fresh.rs` runs `python3 dist/sdk/gen-widget-docs.py --check` and fails, naming the files, when either is stale. It also fails when a new module of the SDK is in no reference page.
- The commands in `first-app.md` were run when the page was written; the dated "Verified" lines say what was run and where.

## Claude Code skills

The repository carries skills for coding agents: `.claude/skills/iyakki-app-development` (make, test and review an app), `.claude/skills/iyakki-sdk-component` (add a new embeddable component) and `.claude/skills/iyakki-feature-rows` (write feature-matrix rows). They follow this guide and point back to it.
