# Alpha scope: 0.1.0-alpha.1

## Status on 2026-10-19

This document was derived on 2026-10-17 and its class tables below are **not rewritten**; read them with this update. The alpha is committed and
tagged today as a preview for developers and testers, not for production.

* **First real hardware.** On 2026-10-18/19 the install image (macweb variant) booted on a Mac mini 2018 (Macmini8,1, T2) from a USB SSD. Class B
  checks that passed there: boot and the boot screen, the data volume and persistent `/home`, HDMI and built-in audio (`apple-bce`), a USB
  webcam and the CAMERA app, Quick install (k3s dev cluster, Elasticsearch, DuckDB, Node.js, Python 3.13, `ffmpeg`), Claude Code and Copilot CLI
  install and sign-in, Lemonade on the LAN, and the full web engine on Google and YouTube. Evidence per item: ALPHA-GATE.md items 46 to 54.
* **Still class B and open, or failing:** Wi-Fi and Bluetooth of the Mac mini 2018 (not working: Apple's firmware and `BCM.hcd` are missing);
  H.264/AAC video (not shipped on purpose); Copilot full tool use, Autopilot with external agents, tile-app publishing against a k3s store,
  the in-cluster registry one-click, Developer mode (all loopback or fakes only); Secure Boot on real hardware (verified in QEMU only); T2
  MacBook Pro 2019 (never booted); Apple Silicon (VM only); bare-metal PCs; live cloud assistants without keys. See [KNOWN-ISSUES.md](../KNOWN-ISSUES.md).
* **Tests.** Last full `cargo test --workspace` on 2026-10-18 evening: 3,845 passed, 0 failed, 59 ignored. A final full run follows before the tag.


Derived from `FEATURES.md` on 2026-10-17 (done 242 · partial 143 · planned 55 · blocked 3 · deprecated 1 · total 444). Every one of the 202 rows that is not `done` is classified below. Quotes in the "Row says" column are the row's own Notes; nothing is guessed beyond the class and size.

**Reading this document.** "100% of the alpha" cannot mean 443 rows done: hardware, Secure Boot, Apple Silicon, payments and a hosted catalog are outside what code alone can deliver. It means: class A is closed in code, class B is verified by hand and written into `ALPHA-GATE.md`, class C is labelled as planned in README/KNOWN-ISSUES, class D is decided by the owner.

## Summary

| Class | Meaning | Rows | partial | planned | blocked | deprecated |
|---|---|---|---|---|---|---|
| A | Close for alpha in code (tests, no hardware) | 42 | 41 | 0 | 0 | 1 |
| B | Needs real-world verification (code exists) | 82 | 80 | 2 | 0 | 0 |
| C | Post-alpha (label as planned) | 71 | 18 | 51 | 2 | 0 |
| D | Owner decision / input needed | 7 | 4 | 2 | 1 | 0 |
| | **Total** | **202** | 143 | 55 | 3 | 1 |

Class A has 37 real code items plus 5 that are status-only corrections (XS). Several class-A rows (for example F-VAULT-05, F-IDE-01, F-EXTA-03, F-EXTA-05, F-MEDIA-03, F-TALES-09, F-SHELL-20) also keep a class-B live check for the same row; the work column says so. Class C includes 4 rows to deprecate or merge rather than build (F-DEP-03, F-WEB-07, F-BROW-02/F-BROW-40).

Honest reading of the alpha: after class A and the class-B checklist, 100% of what *can* be proven before release is done; the remaining partial/planned rows are listed as known gaps, not hidden.

## A. Close for alpha (code, with tests)

Size: XS status only, S under half a day, M about a day, L several days. "Parallel" names the conflict area; rows in the same area go to one agent.

| ID | Work (concrete) | Row says (missing) | Size | Parallel / conflict | Files likely touched |
|---|---|---|---|---|---|
| F-DISP-02 | Audit every Settings/launcher page at 150% and 200% on 800x480 and 1024x600 and make overflowing pages scroll or wrap; add a render test per page. (Resolution half stays planned.) | At very large sizes on a small screen, some pages overflow (they scroll or cut off; each page was drawn at the sizes the tests use). The resolution half stays planned... | M | shell UI pages | shell/src/uisize.rs, apps/settings*.rs, ui.rs |
| F-VAULT-05 | Widen the chat input guard (more key formats, PEM variants, JWT, connection strings) and add negative/positive tests; live check is class B. | The guard is a heuristic (key formats, private key blocks, values after password:/token=): it can miss unusual formats. Verified with scripted models only. The request... | S | independent | agent/src/credguard.rs, assistant_guard.rs |
| F-ASSIST-21 | Pass the workspace test command into the chat (`ProjectTools::with_test_command`) and build the http check, so test and app-responds rows get the automatic check. | The test command is only run when the workspace names one (`ProjectTools::with_test_command`; the chat does not pass one yet) and the http check is not built, so rows... | M | conflict: agent chat/ide | agent/src/boardtrack.rs, autopilot.rs, apps/chat.rs |
| F-ASSIST-22 | Add a Settings screen for `think_limit_secs`/`silence_limit_secs` (and optionally token rate in the footer). | A server that sends no byte at all keeps the HTTP read blocked until the client's own timeout (the run itself moves on at once; the helper thread ends when the read... | S | conflict: settings.rs | shell/src/apps/settings.rs, agent/src/settings.rs |
| F-ASSIST-24 | Settings/IDE switch for `no_workspace_history`, a Forget button, and call `forget_session` when a chat is deleted. | Only Autopilot steps are recorded (plain chat turns already have the per-session memory); there is no Settings screen or IDE button for the switch and for forgetting yet... | M | conflict: chat.rs/ide | agent/src/history.rs, apps/chat.rs, ide.rs, settings.rs |
| F-ASSIST-25 | Add the missing click test for the Mark done button on a review card. | Before: the user's run called board_update 80 times in one step; now it runs twice and the next row starts. The Mark done button is drawn and wired but has no click... | S | independent (ide tests) | shell/src/apps/ide_tests.rs |
| F-IDE-05 | Show a header total of elapsed time on the board. | The repository's own FEATURES.md is unchanged and has no time columns. A header total of elapsed time is not shown yet. | S | conflict: board.rs | shell/src/apps/board.rs |
| F-IDE-06 | Add a settings-form field for `commands.coverage` in project.json. | Parsers are checked on sample output, not on live runs of every tool. `pytest -q` prints no test names, so only `-v` or `-rA` output maps. Coverage is line coverage... | S | conflict: ide settings | shell/src/apps/ide.rs, ide_ui.rs |
| F-DATA-03 | Render data_query results as a table block in the chat and add an "Open in Data viewer" button. | The chat shows the result as plain text in the tool card (a Markdown table), without a table block or an "Open in Data viewer" button yet | M | conflict: chat.rs/chat_view.rs | shell/src/apps/chat_view.rs, data.rs |
| F-EDIT-14 | Enter between a bracket pair puts the closing bracket on its own line with the caret indented between; skip brackets inside strings/comments. | Brackets inside strings and comments count like any other. Pressing Enter between a pair does not put the closing bracket on a line of its own with the caret indented... | S | independent (iyakki-edit) | edit/src/buffer.rs, syntax.rs |
| F-EDIT-16 | Add a way to stop a running file and feed input; test runner for `cargo test`/`pytest` output. (Container path stays class B.) | The container path is only tested up to the explanation of what is missing (the cluster itself is covered by F-CLI-03). No test runner for `cargo test` or `pytest`, no... | M | conflict: edit.rs/ide | shell/src/apps/edit.rs, agent/src/exec.rs |
| F-EDIT-17 | Move iyakki-gui to iyakki-edit directly, delete the legacy line-diff/Rust-highlight shim in iyakki-ui and the row. | Replaced by F-EDIT-05 and F-EDIT-06; remove once `iyakki-gui` uses `iyakki-edit` directly. | S | independent | crates/iyakki-ui, crates/iyakki-gui |
| F-IDE-01 | Implement "Clone from a URL" (git clone through the runner, tested with a local bare repo) instead of the stub. Keyboard/IME/visual claims stay class B. | The terminal is a command runner (one program, no shell, not a pty); "Clone from a URL" is a stub that explains what to do; Ctrl+1..5 and Ctrl+` cannot be delivered by... | S | conflict: ide*.rs | shell/src/apps/ide.rs, ide_tree.rs |
| F-IDE-10 | Ask before closing the IDE window while tasks run. | Not run with a live model or a vendor CLI: tests use scripted models, a fake runner and a fake `claude` script, with real git in temp folders. A task cannot read the... | S | conflict: ide_tasks*.rs | shell/src/apps/ide_tasks.rs |
| F-EXTA-03 | Delete the vendor session-id file when a chat is deleted. | The session id file is not cleaned when a chat is deleted. Neither Claude Code nor Copilot CLI documents a command that lists models without a session (checked... | S | independent (agent/extagent) | agent/src/extagent, sessions.rs |
| F-EXTA-05 | Persist the Autopilot cost total across Resume; add a footer-text test. | Fake CLI only (the MCP tests need python3 and skip without it); no live vendor run. The cost total is not saved across Resume. No one-click chip to switch an advisor... | S | independent (autopilot) | agent/src/autopilot.rs |
| F-APPS-02 | Install a skill package from a store index entry and add the Settings page (list, update, remove). | From a file through the terminal (`store show/install/list/remove/keys/trust`, `skill list/show/add/remove`). Fetching from a store index and a Settings page are still... | M | conflict: store*.rs | agent/src/store, pkg.rs, shell/src/apps/skills.rs |
| F-APPS-03 | Count tool file writes against the run file budget; add update check; test with a module compiled from Rust (needs wasm32 target only, no network). | An installed tool package is offered to the model as `pkg_<name>` (a dot is not allowed in model tool names) for people whose account has a command line, personal ones... | M | conflict: store*.rs | agent/src/wtool.rs, pkg.rs |
| F-APPS-40 | Status only: notes list no missing work (TLS is the ingress job by design); consider done. | TLS is the ingress's job (documented). Yank hides a package from catalogs; it does not disable copies already installed (a root-signed revocation list does that). | XS | independent |  |
| F-APPS-41 | Status only: notes list no missing work; consider done. | STORE, Settings, Private store card: Deploy to k3s, Add a private store, Publish here. | XS | independent |  |
| F-APPS-43 | Status only: notes list no missing work; consider done. | docs/APP-AUTHORING.md. | XS | independent |  |
| F-APPSTORE-10 | A CLI item should open the CLI tile on its entry. Bare-metal drawing check is class B. | NOT seen on bare metal yet: drawn and clicked at 1280x720, 1920x1080, 1024x600 and 800x480 in tests only. A CLI item opens the CLI tile but not on its entry. | S | conflict: store.rs | shell/src/apps/store.rs, cli.rs |
| F-BROW-01 | Note is stale (pictures and forms exist elsewhere); add find-in-page and in-page anchors, then close. | Not tried on real sites yet. Missing: pictures, forms, in-page anchors, text search, find-in-page. | S | conflict: browser.rs | shell/src/apps/browser.rs |
| F-BROW-05 | Animated WebP frames, GIF loop count, `<picture>`/srcset choice, lazy loading. | PNG, JPEG and GIF are done (F-BROW-13). Animated WebP shows only its first frame. SVG is checked by hand-computed areas, widths and pixels (no reference renderer is... | M | browser_images.rs (independent) | shell/src/apps/browser_images.rs, iyakki-web |
| F-BROW-07 | Status/note stale: F-BROW-31 says persistent cookies are kept in the vault; fix the note and mark done. | Cookies live for the session only; nothing is saved to disk yet. | XS | independent |  |
| F-BROW-14 | Status/note stale: the note says pages do not use style sheets but F-BROW-06 is 85% and uses them; mark done. | The style sheet switch is saved but pages do not use style sheets yet (F-BROW-06). | XS | independent |  |
| F-BROW-22 | Streamed download with a progress bar, open a file from the Ctrl+J list, cancel stops the fetch. | No progress bar: the file is fetched in one piece, so very large files are refused rather than streamed. Cancel stops it being saved, not being fetched. A list entry... | M | browser.rs | shell/src/apps/browser.rs, browser_data.rs |
| F-BROW-23 | Bookmark folders (create/move), search box on the history page. | No screen for making folders or moving bookmarks (folders arrive only from an import), no search box on the history page (the address bar's suggestions search it), no... | M | browser.rs | shell/src/apps/browser_data.rs |
| F-BROW-24 | Remove a saved sign-in from the browser. (Passkeys and name/address fill are class C.) | Not done: filling names and addresses, passkeys (WebAuthn), filling by clicking, removing a saved sign-in from the browser (Settings → Vault lists them as... | S | browser_logins.rs (independent) | shell/src/apps/browser_logins.rs |
| F-BROW-25 | Ctrl+scroll zoom and a text-only size. | Not done: a text-only size, Ctrl+scroll. | S | browser.rs | shell/src/apps/browser.rs |
| F-BROW-29 | Favicons and inline `<svg>` in pages (reuse the existing SVG rasteriser). Canvas 2D, AVIF, APNG are class C. | Extends F-BROW-05; canvas is what charts and map tiles use. Not started: `<canvas>`, inline `<svg>` elements (the page reader skips them), AVIF, APNG, favicons. | M | browser_images.rs | shell/src/apps/browser_images.rs, iyakki-web |
| F-BROW-34 | Certificate details view and mixed-content blocking for scripts and styles. | Not done: certificate details, a list of known-bad sites, mixed content for scripts and styles, a way to continue past a bad certificate (deliberately none). | M | browser.rs | shell/src/apps/browser.rs, iyakki-proxy |
| F-BROW-36 | ETag/Last-Modified revalidation in the disk cache. | Revalidation (ETag) is not built. The script interpreter itself (QuickJS in WebAssembly; build flags, link-time optimisation, bulk memory, a faster allocator and... | M | independent (iyakki-web cache) | crates/iyakki-web, agent/src/webpage.rs |
| F-BROW-37 | User-defined address-bar short words in Settings → Browser. | No suggestions from the search engine itself (that would send each keystroke over the network), and no way to add your own short words yet. | S | browser.rs | shell/src/apps/browser_config.rs |
| F-BROW-39 | Read the page aloud, and copy a screenshot of an area. Live-provider check is class B. | The panel and the keys are tested with a stand-in model; not tried against a real provider here. | S | browser_assist.rs | shell/src/apps/browser_assist.rs |
| F-BROW-41 | Script console prompt and a "report a broken site" button (inspector is class C). | Console prompt, inspector and the report button are not built. | M | browser_devtools.rs (independent) | shell/src/apps/browser_devtools.rs |
| F-MEDIA-03 | Button in the MEDIA app for the existing assistant tagging helper. Real embedding model is class B. | In-house, no database engine (see docs/MEDIA-SEARCH.md). Persisted per user, incremental by size and modified time. Vectors come from the configured embedder (hashed... | S | media.rs | shell/src/apps/media.rs |
| F-MEDIA-04 | Advance an external-player playlist (the app does not today); tested with a stand-in player. | Offered when the role has MEDIA and the command line is not off; no approval (reads the library, plays files inside the read scope). Tested with a scripted model and a... | M | media.rs | agent/src/media.rs, shell/src/apps/media.rs |
| F-MEDIA-05 | Resume a half-done download (Range requests), tested on a loopback server. | Direct file links only (host policy first, at every redirect; write scope; size cap, default 2 GB; first bytes checked; web pages and video sites refused; never... | M | independent (mediadl.rs) | agent/src/mediadl.rs, mediaqueue.rs |
| F-FLOW-11 | List and install flow templates from the STORE screen (hook and manifest already exist). | Partial: the Store's own screen does not list or install flow templates yet (the hook and the manifest target exist). | M | conflict: store*.rs | shell/src/apps/store.rs, flows.rs |
| F-TALES-09 | Use the browser voice for chat replies read aloud (today only TALES uses it). | Verified with a fake page in tests and the page's real JavaScript in a real headless Chrome (an ignored test; voices listed, a piece spoken after a real click, start and... | S | conflict: story.rs/chat.rs | shell/src/apps/story.rs, chat.rs, agent/src/speech.js |
| F-SHELL-20 | Fence framebuffer/evdev/boot code with `cfg(target_os)` so the shell, agent and core `cargo check --target aarch64-apple-darwin` pass (needs the rustup target; Apple C toolchain may block linking). Running it is class B (Mac). | Done and tested: an Ollama already answering on this machine's usual port is added to Models as `ollama-local` when an administrator signs in (also helps Linux desktops... | L | conflict: shell.rs/lib.rs (touches many crates) | shell/src/lib.rs, fb.rs, input.rs, core/src/main.rs, boot.rs |

## B. Needs real-world verification

Code exists and is tested with fakes or scripted models. The exact manual steps are in the checklist (section E); this table says which environment each row needs.

| ID | Feature | Row says it is unverified | Env |
|---|---|---|---|
| F-HW-02 | Real microphone / camera capture (cpal, v4l features) | Compiles; never run against real devices. | AUDIO,PC |
| F-NET-02 | WiFi configuration (wpa_supplicant planner) | Never associated with a real access point. | WIFI |
| F-NET-07 | Settings → Network (administrators) | NOT tried with a real WiFi adapter or access point: the logic is tested with `iw` output in its real shape and stand-in programs, the key derivation against the standard's test vectors. Not done: WPA... | WIFI |
| F-PKG-14 | Run on other base operating systems | Tested as an ordinary program in containers and as a service on this computer's systemd; the framebuffer desktop, driver loading and the container cluster still rely on the Buildroot image (a... | PC |
| F-SHELL-09 | Desktop window (egui) for development | Separate workspace; compiles, no automated UI test, needs a display. | WIN |
| F-IDE-07 | Bugs on the Board | The skills' bodies are at the 3,950 character limit (k3s exactly). The Bugs lane colour is the red stripe and id of the card, not a separate lane. | WIN |
| F-DASH-06 | DASH tile (Work group) | Drawn and driven in tests at 1280x720, 1920x1080, 1024x600 and 800x480; not yet tried on bare metal or with a real server and mouse. Heatmap, pie and donut are simple; no alerting, annotations, share... | WIN |
| F-FLOW-09 | FLOWS tile (Work group, three-box icon, accent violet) | Partial: drawn and clicked only in tests, not looked at in a real window; the text view is JSON (no YAML library); a card's pending-run update banner shows the first step only; no middle-button pan... | WIN |
| F-IDE-09 | Browser tab in the AI IDE | Not seen on a real screen or with real WebKit/WPE (only the fake helper and the reader); the bottom-panel PREVIEW tab was left out on purpose (one surface); `readonly` ignores the person's page input... | WIN |
| F-VOICE-06 | Browser voice helper for the desktop window | Tested with a raw WebSocket client standing in for the browser; the page's JavaScript is the one the web front end already ran in a real headless Chrome, but the helper page itself was not opened in... | WIN |
| F-SHELL-13 | Responsive display loop | Measured only in QEMU; confirm on the Mac mini. Software rendering is still single-threaded. | MAC |
| F-BOOT-13 | Boot screen | Not yet seen on the Mac mini; the logo file was supplied by the owner. | MAC |
| F-BOOT-08 | aarch64 image for VMs (Apple Silicon Macs via UTM/QEMU,... | Image builds; boots through UEFI and GRUB to IYAKKI in QEMU (self-test passes, framebuffer found). Never run on a Mac. | MAC |
| F-PKG-06 | USB writer for Linux and macOS (iyakki-usb) | Linux path tested end to end with a stand-in file and menu/warning on a real USB stick. The macOS path (diskutil parsing) is tested only on sample text, never on a Mac. | MAC |
| F-DRIVERS-03 | Firmware on the data volume and the boot hook | Not seen on a real Mac or in QEMU: whether the kernel follows the symlinks for a given driver and whether `modprobe -r` succeeds are unverified. Drivers that load before the data volume (storage,... | MAC |
| F-DRIVERS-05 | FaceTime HD camera firmware extracted in Rust from the... | The checksum table was transcribed from the upstream script through a summarising tool and is unverified (a wrong value fails closed); it was only tried against synthetic files; Apple's disk image is... | MAC |
| F-DRIVERS-07 | STORE, Installed | NOT seen on bare metal: drawn and clicked in tests only. | MAC |
| F-AI-02 | Live verification against OpenAI / Claude / Gemini | Needs real API keys; only Ollama has been exercised live. | KEYS |
| F-ASSIST-10 | Assistant robustness | Checked against scripted models and live against Ollama (qwen2.5-coder:7b stops printing its call as the answer). The phrase tables are English only; the rolling summary is extractive (one line per... | OLLAMA |
| F-ASSIST-11 | Assistant speed | Measured with `assistant_bench` on a live qwen3:8b: 3.3-5.6 s per question became 1.4-2.4 s and the first request went from 8.7k to 3.5k tokens (docs/ASSISTANT-ROBUSTNESS.md). Gemini is not streamed;... | OLLAMA |
| F-ASSIST-14 | Stall guard and actionable file errors | Verified with scripted models and temp folders only, not yet with a live qwen3:8b. The phrase tables are English only; the nudge is a heuristic and can miss an odd phrasing or fire on a status line.... | OLLAMA |
| F-ASSIST-15 | The assistant plans on the feature board in IDE workspace... | The board tools were always offered (they sit in the always-on extra group); what was missing was the instruction and a way to create the file. Verified with a scripted model; whether qwen3:8b... | OLLAMA |
| F-ASSIST-17 | Board status tracker | Heuristic: rows are matched by words in their title (build, load/push/registry, deploy/manifest/service, verify/test) or the file written, so an oddly worded row stays for the model or the person.... | OLLAMA |
| F-ASSIST-20 | Done needs proof | A done set by hand in the file or the Table view cannot be told from a forced one, so it shows as claimed in the Autopilot summary. Rows the person wrote are never marked done by the model. Scripted... | OLLAMA |
| F-ASSIST-23 | Autopilot context compaction and token budget | The summary is extractive (no model call), so it is exact but terse. Token counts are the estimates the run already uses unless the provider reports usage. The footer shows the last step's tokens in... | OLLAMA |
| F-ASSIST-26 | Autopilot fixes instead of advising, and reports only what... | The advice detector is a phrase list (English), so unusual wording may pass as an answer; a model that never acts is stopped by the no-progress budget before its row is blocked unless that budget is... | OLLAMA |
| F-ASSIST-27 | Tool calls printed as pretty-printed JSON are run, one row... | The test-name heuristic treats a capitalised or long sentence as a criterion, so a jest title written as a sentence must be listed in lower case or with a path. Existing duplicate rows are not... | OLLAMA |
| F-ASSIST-28 | The assistant does it itself | Phrase lists are English only. The skills block is in every compact step request (checked with a marker in the prepared context). Scripted models only. docs/ASSISTANT-ROBUSTNESS.md. | OLLAMA |
| F-ASSIST-29 | No questions once the goal is done | A question in prose that the model writes in a turn before it calls `ask_user` cannot be taken back once shown (the goal's last step is the one guarded). The "once per chat" memory is the chat... | OLLAMA |
| F-IDE-08 | Every workspace run starts from the project's state | The stack is guessed from marker files (and `.c`/`.cpp` files next to a Makefile or CMakeLists), so an unusual layout may need the project setting. Deploy facts come from what the tools returned in... | OLLAMA |
| F-ASSIST-18 | Autopilot for the AI IDE chat | Scripted models only; not yet driven live with qwen3:8b on a real build and deploy. Only the AI IDE chat has it (the stand-alone CHAT does not); the saved place is per workspace, not per chat; the... | OLLAMA |
| F-ASSIST-19 | Auto mode does not ask permission | Phrase tables are English only. The reachable line needs the node's InternalIP (an unknown node address is said so). Not yet watched with the user's qwen3:8b. | OLLAMA |
| F-DASH-08 | The assistant inside the DASH tile | Run with a scripted model only; not tried with a real model, so how well it writes queries and explains a spike is unmeasured | OLLAMA |
| F-TALES-16 | Built-in cartoon composer for small models | These are cartoons, not realistic pictures (realism needs a picture model); people have one front view with a few poses; how apt a real qwen3:8b's scene choice is was not judged (fakes only). Contact... | OLLAMA |
| F-FLOW-06 | Agent-monitored healing | Partial: the built-in model is the only healer (an external agent through the executor seam is not wired); the CLI install itself goes through the real installer, tested only with a fake host; no... | OLLAMA |
| F-DASH-04 | DuckDB connector through the duckdb command (not linked) | Tested with the scripted runner only: the command-line flags come from DuckDB's documentation and the CLI catalog entry's download name (`duckdb_cli-linux-<arch>.zip`, no checksum published) were not... | DUCKDB |
| F-BUG-01 | Bugs as board rows with captured evidence | Evidence is read-only `kubectl` output through the existing runner; with FakeRunner answers only. The signature matching for the harness' own reports uses the reasons and the pod name seen in the... | K3S |
| F-IDE-02 | AI IDE cluster view | Read paths are tested against a fake executor and, in an ignored test, this machine's real k3s (read-only); cluster profiles are a kubeconfig and context in `.iyakki/project.json` and a per-user... | K3S |
| F-K8S-02 | Deploy flow | The local registry and its k3s mirror file are set up by the person (no root helper); no push to a registry for remote clusters, no rollback, Secrets from the vault or namespaced service account yet;... | K3S |
| F-K8S-06 | Registry choice for the k3s a workspace uses (regsetup) | Nothing was applied to a real cluster by the tests (FakeRunner); the ignored live test only reads the current cluster; the pod pull via `localhost:<nodePort>` needs kube-proxy to expose NodePorts on... | K3S |
| F-K8S-07 | Which cluster a workspace talks to, with no leaks | Hiding is per kubeconfig FILE (all its contexts); a plain `kubectl` typed in an external terminal outside IYAKKI still follows your environment; `docs/K3S-DEV.md` | K3S |
| F-K8S-08 | Tear down the workspace's dev environment | Tested with a fake kubectl and curl, never against a real cluster; the registry cleanup needs a registry that allows deletion and the disk space is only freed by its garbage collection; in chat the... | K3S |
| F-K8S-10 | Test containers before and after the deploy | Fake docker/kubectl only; the output parsers read the common summaries (other frameworks give counts as not recognised); the wait polls the Job status every 3 s instead of kubectl wait;... | K3S |
| F-K8S-12 | Settings → Dev cluster (administrators) | Tested with a fake `kubectl`; never run against a real remote cluster. The page is for administrators; ordinary accounts keep choosing in the AI IDE's Cluster… chooser. The registry apply (A, B, C)... | K3S |
| F-K8S-16 | Platform services engine (docs/PLATFORM-SERVICES.md) | Tested with a fake runner, fake HTTP and an in-memory vault; never run on a real cluster. Pinned image tags and Gitea command flags come from the documentation and are unverified; digests are not... | K3S |
| F-K8S-17 | Gitea as a platform service | Never run against a real Gitea. No persistent git credential helper (pushes made by IYAKKI use the vault token; your own terminal uses SSH); task-branch pull requests are documented, not wired. | K3S |
| F-K8S-18 | Platform services UI and assistant tools | The K8S panel button opens the same panel; tested at 1280x720, 1920x1080, 1024x600 and 800x480 with fakes only. | K3S |
| F-APPS-42 | Private store as a platform service (private-store) | Not run against a real cluster. | K3S |
| F-FLOW-04 | Node kinds as a registry (NodeKind | Partial: agent steps use the configured model only (no tools, no external agent yet); the browser step is tested with a fake host and has never run on a real page; thrift is F-FLOW-14. | K3S |
| F-FLOW-05 | Runners | Partial: the Job path is tested with a fake kubectl only; never run against a real cluster. The remote runner is F-FLOW-16. | K3S |
| F-K8S-13 | Bare-metal "Set up a local cluster" | Tests use a stand-in `k3s` script and a fake downloader: nothing was downloaded and no real cluster was started. NOT yet run on the bare-metal box: the real download, the RAM-root jail with this data... | PC,K3S |
| F-K8S-14 | Cluster health in SYSTEM and alerts | Alerts wait in memory until an administrator is signed in. The pod count and Ready state come from `k3s kubectl`; checked here only against a stand-in. | PC,K3S |
| F-CLI-03 | Container cluster on the machine itself | Tried with a real k3s 1.37 (2026-10-11): in a privileged container and inside the booted x86 image under QEMU, python and shell sandboxes run as uid 65534, the system folders are read-only, a 1 GB... | PC,K3S |
| F-K8S-15 | Cloud mode (docs/K3S-DEV.md phase 5) | Never run inside a real pod or against a real EKS/GKE kubeconfig. The vault resolver is installed at sign-in (one signed-in person per process); background kubectl calls of another person are not... | CLOUD |
| F-WEB-06 | Headless multi-user server mode | Tried on a real K3s (the Deployment started, the init container made the account from the Secret, sign-in worked through the Service) and in a read-only Docker container with a data volume. Per-user... | CLOUD |
| F-VOICE-04 | Verification on a real microphone and speaker | Only fake devices and QEMU audio were used. | AUDIO |
| F-VOICE-05 | Reading aloud with no setup | Tested with fake engine scripts on a temporary PATH (each kind, hostile text, cancellation) and detection run on this computer (only speech-dispatcher with its espeak-ng module is installed here;... | AUDIO |
| F-TALES-02 | Reading aloud | Tested with a script as the speech engine and as the speaker; no real sound was heard (no audio device in the test setup). Check on bare metal with Settings → Voice → test speaker, then a short kids'... | AUDIO |
| F-MEDIA-01 | Media center | MEDIA tile in the RADIO look (glowing now-playing card, orb, round transport, progress, cosmetic level bars that move only while playing; kuthu magenta, melody cyan, beats amber): per-user library... | AUDIO |
| F-EXTA-01 | External coding agents, adapters (docs/EXTERNAL-AGENTS.md) | Tested only against fake CLIs: the flags are from the vendors' documentation (read 2026-10-14) and no live vendor run has been made. Copilot's JSONL event shapes, `--additional-mcp-config` and a... | AGENT |
| F-EXTA-02 | Policy bridge for external agents | Not run against a live Claude Code: whether it sends `_meta` with the tool-use id and the exact permission-prompt input are from its documentation. The user's project `.claude/settings.json` can... | AGENT |
| F-EXTA-04 | Settings → Agents (administrators) | The sign-in itself is not started from IYAKKI (the vendors' sign-in is interactive): the page shows the command to run in a terminal. | AGENT |
| F-EXTA-06 | Coding agents in the CLI tile's catalogue (Claude Code,... | Tested with stand-in node and npm only; no real install was run. The vendors' native curl-to-bash installers are not offered. The IDE workspace terminal does not search the tile's bin folder. Package... | AGENT |
| F-JOBS-04 | External coding agents (Claude Code and others) get the six... | Tested through the real gate, not with the vendor's program: NOT tried with a real Claude Code session. The agent's own foreground shell tool cannot be switched off from here; it is only told not to... | AGENT |
| F-TALES-11 | TALES with an external agent | Verified with scripts that print the documented stream shapes, not against a live Claude Code or Copilot CLI; the agent's own tool switches (`--disallowedTools`) are the CLI's documented ones, and... | AGENT |
| F-TALES-15 | TALES pictures are drawn by the writer the person chose | Run with fakes only: how good a real Claude or GPT drawing is was not judged, there is no vision check of the rendered picture (the chat protocol carries no images), and the real agent path... | AGENT |
| F-STORE-04 | Full install to disk with a bootloader | Works for USB sticks and other drives through `iyakki-usb --install` (add `--all` for internal disks); the installer inside the running OS is F-INST-01 (`iyakki install`, Settings → Install to disk). | PC |
| F-BOOT-01 | Boot on real PCs | Hardware check and test guide (`docs/BOOT-TEST.md`) are ready; waiting for results from real machines. Only QEMU has been tested. | PC |
| F-SYS-03 | Remote diagnostics | Needs the image built with `IYAKKI_SSH_AUTHORIZED_KEYS`; not yet tried on hardware. | PC |
| F-ZRAM-01 | zram swap set up by IYAKKI at boot, no systemd | Details: `docs/ZRAM-AND-INSTALL.md`. Kernel options checked with `verify-kernel-config.sh` for x86-64, the Mac variant and aarch64. Not seen on real hardware. | PC |
| F-ZRAM-02 | Optional swap file on the data volume (iyakki.swapfile=2G,... | No hibernation. zswap needs a swap file to be useful; only the switch is tested (fake sysfs). | PC |
| F-INST-01 | Install to this computer from the booted stick | MBR layout (BIOS and UEFI from one disk) or GPT (F-INST-03). Not run on a real computer. Planned: see F-INST-04. | PC |
| F-BROW-04 | Full web engine for JavaScript-heavy sites (YouTube,... | Window mode on a development machine and the `web` image in QEMU both work (see F-BROW-43); `qemu-web-test.sh` needs a built web image, QEMU and sudo, so it is run by hand, not by `cargo test`. The... | IMAGE |
| F-BROW-43 | Embedded full web engine | Tests use a pure-Rust fake helper; the image's WPE helper is checked by `dist/buildroot/qemu-web-test.sh` (by hand: needs the built web image and QEMU); the real WebKitGTK helper was exercised by... | IMAGE |
| F-BROW-44 | Supervised browser automation | Design and threat model: `docs/BROWSER-AGENT.md`. Live checks (ignored): `cargo test -p iyakki-agent --lib browserlive -- --ignored --nocapture --test-threads=1` ran the WebKitGTK helper against a... | IMAGE |
| F-MEDIA-07 | Sample media in every image and video on the web image | Files in `dist/samples/` (CC0, `make-samples.py`), installed by `post-build.sh` into `/usr/share/iyakki/samples` of both images. MEDIA scans that folder as a read-only source tagged `sample`, never... | IMAGE |
| F-MEDIA-06 | Download queue | `mediaqueue.rs`: FIFO, at most 2 at a time per person (tested against a local server that counts open requests: the maximum seen is 2), entries numbered, saved in `media-downloads-<user>.json` (an... | NET |
| F-TALES-13 | TALES pictures are automatic with images first | Image generation itself was only tested up to the request and its failure (no real image server was available), so the fall-back to SVG is tested for real and a successful image is not. An external... | IMGMODEL |
| F-TALES-18 | TALES pictures are format-agnostic | No real image or video model was available: only fakes and 8x8 sample files were tested. No video generation API is called, no video player or poster frame is drawn (the slot shows a card), a PNG is... | IMGMODEL |
| F-CAM-01 | Camera discovery on the user's own LAN (ONVIF, mDNS, SSDP);... | Never scans outside the local subnet; no automatic connection. | CAM |
| F-REL-01 | Version everywhere | The `/etc/iyakki-release` step has not run on a built image yet (the images in `target/` predate it) and About was not looked at on a screen. | BUILD |
| F-REL-05 | ./run.sh release [version] | Never run end to end: it needs freshly built images. `--dry-run` and the signing library are tested. | BUILD |

## C. Post-alpha

Label every row below as "planned" in README and KNOWN-ISSUES (the table says whether it needs a dedicated line: all do unless marked "no").

| ID | Feature | Reason | Target | Label in KNOWN-ISSUES |
|---|---|---|---|---|
| F-WASM-06 | Copy-on-write snapshots (sub-millisecond forks) | Performance optimisation; snapshots work today by copying linear memory. | 1.0 | yes |
| F-WASM-07 | Snapshot in the middle of a guest call | Needs an engine-level capability; no alpha user need. | 1.0 | yes |
| F-MEM-02 | External vector store for L3 (LanceDB / Qdrant-lite) | The built-in cosine store is sufficient at alpha scale. | 1.0 | yes |
| F-NET-04 | Swarm scheduler beyond "first connected peer" | Swarm scheduling needs several real peers. | beta | yes |
| F-NET-05 | Bluetooth userland | Bluetooth userland is a large driver/userland job. | 1.0 | yes |
| F-PKG-04 | hw image with firmware blobs for WiFi/NIC/GPU | Firmware-blob image needs a licence review and hardware to test; never built. | beta | yes |
| F-SHELL-10 | Direct DRM/KMS + GBM renderer | DRM/KMS renderer is a large rewrite; fbdev works and is the alpha path. | beta | yes |
| F-SHELL-12 | Touchpad support on bare metal | Touchpad on bare metal needs real hardware work. | beta | yes |
| F-STORE-05 | Encryption of the data volume | Data-volume encryption; vault entries are already encrypted. | 1.0 | yes |
| F-STORE-06 | System image as a read-only compressed filesystem (EROFS or... | Needs kernel config (EROFS/SquashFS/OVERLAY) and an image rework; part of F-BOOT-14. | beta | yes |
| F-STORE-07 | Data volume filesystem | Evaluation of F2FS/Btrfs; ext4 works. | 1.0 | yes |
| F-WEB-04 | Native web views for Chat, Edit, Runs and Settings | Needs app logic split from drawing code; pixel streaming covers it for alpha. | beta | yes |
| F-WEB-05 | iyakki-edit compiled to WebAssembly for the browser editor | Browser editor in WASM; pixel streaming covers it for alpha. | beta | yes |
| F-WEB-07 | Terminal over WebSocket (CLI tile) | Note says the pixel-streamed CLI tile already covers it; recommend deprecating the row. | beta | no, deprecate/merge row |
| F-DEP-03 | Replace thiserror derive with hand-written errors | Note says low value (compile-time only); recommend deprecating, not building. | never | no, deprecate/merge row |
| F-BOOT-02 | Secure Boot support, route 1 | Secure Boot route 1 is decided "later, after hardware testing"; needs key custody (owner). Alpha README must say: turn Secure Boot off. | beta | yes |
| F-BOOT-04 | Lazy driver loading through a device broker (audio, GPU,... | Device broker; all matching drivers load at boot today. | beta | yes |
| F-SYS-01 | SYSTEM tile and iyakki top | Remaining items (WASM tasks as rows, per-IYAKKI-user view, per-process network) are enhancements; the tile works. Consider marking done. | beta | yes |
| F-SYS-02 | Hardware view | Load/unload waits for the device broker (F-BOOT-04). | beta | yes |
| F-APPS-01 | Package format | Other package types (CLI commands, agents) are still to come. | beta | yes |
| F-APPS-04 | Signed index + iyakki store CLI (self-hostable, LAN mirrors... | LAN mirrors through the swarm; folders, sticks and remote catalogs already work. | beta | yes |
| F-APPS-05 | STORE tile with permission prompts, ratings and revocation... | Ratings need a service; permission review, trust badges and revocation exist. | 1.0 | yes |
| F-APPS-06 | Optional service apps | Optional service apps model; nothing ships in the base image. | 1.0 | yes |
| F-APPSTORE-16 | Extension taxonomy | Tiles do not yet apply connectors, style packs or agent presets; only WASM tools and themes exist. | beta | yes |
| F-DRIVERS-09 | OPTIONAL Mac image variant (build.sh mac, a future ./run.sh... | Optional Mac image variant; never built or booted, fragment not run through kconfig. | beta | yes |
| F-DRIVERS-10 | Built-in Wi-Fi of the Mac mini 2014 (Broadcom BCM4360) | Partial: no open BCM4360 driver; the owner builds Broadcom's proprietary wl locally (`dist/drivers/build-wl-pack.sh`) and installs it as a signed pack; never shipped, not loaded on a real Mac yet. Ethernet or USB Wi-Fi works. | never shipped | yes (hardware limits) |
| F-DRIVERS-11 | T2 Macs | Partial: the Mac image builds apple-bce (built-in speaker and headphone jack tried by hand on a Mac mini 2018); package not built in an image yet; Bluetooth options not booted; MacBook keyboard/Touch Bar not covered. | 1.0 | yes |
| F-DRIVERS-12 | Apple Silicon bare metal (Asahi-class | Apple Silicon bare metal; documented only. | 1.0 | yes |
| F-DRIVERS-13 | Download Apple's camera driver on the device (after the... | Today the person supplies the extracted file. | beta | yes |
| F-DRIVERS-14 | A small in-house fan controller (what mbpfan does) on top... | Needs the Mac image variant first (F-DRIVERS-09). | beta | yes |
| F-BROW-02 | Browser as an optional package | Duplicate of F-BROW-40. | beta | no, deprecate/merge row |
| F-BROW-40 | Browser as an optional package | Duplicate of F-BROW-02; merge into one row. | beta | no, deprecate/merge row |
| F-BROW-06 | Browser style sheets | Remaining: web fonts, sticky, table styling, nesting. Note is stale: F-BROW-42 says floats, absolute and ::before are done. | beta | yes |
| F-BROW-08 | Browser JavaScript | Missing: ES modules, in-page clicks/typing, Shadow DOM behaviour; the full engine is the alpha route for heavy sites. | beta | yes |
| F-BROW-16 | Video and audio in the browser | Video decoders (H.264/VP9/AV1) permissively licensed are a big job; external player or full engine in alpha. | beta | yes |
| F-BROW-18 | Browser platform layer | Layout sizes, MutationObserver, WebGL; see F-BROW-28. | beta | yes |
| F-BROW-26 | Complex scripts and fonts | CJK and emoji fonts are 10+ MB; English-first font decision. | beta | yes |
| F-BROW-27 | Video playback in pages | Decoder work; see F-BROW-16. Full engine plays VP9/Opus in QEMU. | beta | yes |
| F-BROW-28 | Layout sizes for scripts and responsive pages | Real layout sizes for scripts: large engine change. | beta | yes |
| F-BROW-30 | Interactive form controls | Pickers, file chooser, contenteditable, IME; large. | beta | yes |
| F-BROW-31 | Persistent web storage | Remaining: cache storage and localStorage over ~60 KB; the 64 KiB vault entry limit. | beta | yes |
| F-BROW-32 | Real-time web | Working WebSocket/EventSource/WebRTC; large. | beta | yes |
| F-BROW-33 | Mail, documents and office web apps (Gmail, Outlook, Docs,... | Depends on F-BROW-24/28/30/31/32. | 1.0 | yes |
| F-BROW-35 | Printing and documents | Print to PDF and a PDF viewer; large. | beta | yes |
| F-BROW-38 | Accessibility and input | Accessibility and input; scope and design needed. | beta | yes |
| F-BROW-42 | Standards the engine still lacks. Done | Listed work is largely done; progress 15% looks wrong versus the notes. What remains is long-tail CSS/JS. | beta | yes |
| F-BROW-45 | Deterministic browser test runner, no model | Parallel runs, whole-spec retries, recording. | beta | yes |
| F-RADIO-03 | Radio | AAC/Ogg/HLS decoders (lewton); many stations also have MP3. | beta | yes |
| F-CLI-08 | Interactive SSH sessions in the CLI tile (a pseudo-terminal and a terminal emulator) | The CLI tile runs `ssh`, `scp` and `sftp` without a terminal (F-CLI-05 to F-CLI-07). A real login, `top` or `vi` needs: `openpty`/`forkpty` with `setsid` and `TIOCSCTTY`; `TIOCSWINSZ` and `SIGWINCH` on resize; a VT100/ANSI emulator (cursor addressing, SGR colours, alternate screen, scroll regions, UTF-8 and wide glyphs) with scrollback; drawing it in the tile at every size; raw key forwarding (arrows, Ctrl, function keys, paste); BOUNDARY.md rows for `openpty`, `setsid`, `ioctl`. About 1,800 to 2,500 lines plus tests, most of it the emulator. Details: `docs/CLI-SSH.md`. | beta | yes |
| F-FILE-01 | SFTP server (OpenSSH or Dropbear as a separate program)... | SFTP server; off by default. | beta | yes |
| F-FILE-02 | SFTP/FTPS client app with saved connections (credentials in... | SFTP/FTPS client. | 1.0 | yes |
| F-FILE-03 | Plain FTP | Plain FTP; unencrypted. | 1.0 | yes |
| F-MEDIA-02 | DLNA/UPnP media server and casting | DLNA/UPnP. | 1.0 | yes |
| F-CAM-02 | Live camera view | Depends on F-BOOT-04 and F-APPS-06. | beta | yes |
| F-CAM-03 | Recording with retention limits, encrypted storage and an... | Needs encryption (F-STORE-05). | 1.0 | yes |
| F-CAM-04 | Local motion and person detection (no cloud), zones and... | Local detection. | 1.0 | yes |
| F-CAM-05 | Agent alerts | Vision alerts. | 1.0 | yes |
| F-BOOT-09 | Boot on Apple Silicon hardware (no VM) | Blocked: no Asahi-style support for M3/M4. | never | yes (hardware limits) |
| F-PKG-07 | USB writer for Windows (raw disk access, volume locking,... | Windows USB writer; window mode on Windows is separate. Alpha: write the stick from Linux/macOS. | beta | yes |
| F-PKG-08 | USB writer with a window (drag the image, pick the stick,... | Windows-friendly drag-and-drop writer. | beta | yes |
| F-PKG-11 | Shrink the images (compress kernel modules, drop rarely... | Image size; x86 ISO is 61 MB. | beta | yes |
| F-BOOT-11 | Secure Boot, route 2 | Microsoft-signed shim; needs EV certificate and public review. | 1.0 | yes |
| F-BOOT-12 | Secure Boot, route 3 | Enrol key in firmware; depends on F-BOOT-02. | 1.0 | yes |
| F-BOOT-14 | Two-stage boot | Two-stage boot; needs kernel config and image rework. | beta | yes |
| F-BOOT-15 | Critical-path boot | Critical-path boot. | beta | yes |
| F-BOOT-16 | Start the kernel directly from UEFI (EFI stub) with a small... | EFI stub boot. | 1.0 | yes |
| F-FLOW-14 | Thrift RPC node | Thrift node; refuses to run, marked planned. | 1.0 | yes |
| F-FLOW-15 | Webhook trigger (an authenticated local HTTP endpoint that... | Webhook trigger; `flow_run` with idempotency key is the substitute. | beta | yes |
| F-FLOW-16 | Remote runner (run a node or a flow on another IYAKKI or a... | Remote runner. | 1.0 | yes |
| F-INST-04 | Installer follow-ups | Side-by-side install, encryption, aarch64 installer, legacy BIOS boot on 4Kn. | beta | yes |
| F-EDIT-15 | Inline completions (done | Row mixes done (inline completion, F6) and not done (language server). Split the row; LSP is beta. | beta | no, deprecate/merge row |

## D. Owner decision or input needed

| ID | What is needed |
|---|---|
| F-APPSTORE-02 | Owner must generate and pin the official root key (`official-roots.txt` is empty); decide custody. Until then only image-built items are Official. |
| F-APPSTORE-06 | No catalog is hosted anywhere and the default catalog list is empty; the owner must choose where it lives (HttpFetch path never run). |
| F-APPSTORE-08 | Licence tokens work offline; payment provider and revenue share are a business decision (docs/STORE.md s.10). |
| F-APPSTORE-13 | Payments: provider and revenue share undecided. |
| F-APPSTORE-14 | Who holds the key, where the catalog lives, who reviews Verified publishers. |
| F-PKG-05 | Blocked: the 30 MB / 100 ms targets are not met (ISO ~63 MB, boot time unmeasured). Owner to restate the targets or drop them. |
| F-REL-07 | Owner-run script (`--yes-i-am-the-owner`) that makes the ten commits and tag; the commits are not individually buildable per COMMIT-PLAN.md. |

Also needing the owner (not rows): the Secure Boot signing key custody (F-BOOT-02, class C), the release signing key backup (F-REL-05), and the portal address for F-APPSTORE-19 (a `done` row whose own note says the official address is not set).

## Status corrections to make in FEATURES.md

**Duplicates / overlaps**

* F-BROW-02 and F-BROW-40 are the same text (browser as an optional package). Keep one, delete the other (rows are deleted only with their code, so deprecate one with "Replaced by F-BROW-40").
* F-BROW-04 and F-BROW-43 both describe the embedded full engine and repeat the same measurements. Keep F-BROW-43 as the engine row; reduce F-BROW-04 to the "optional package / store" part or deprecate.
* F-NET-02 (WiFi planner, 70%) and F-NET-07 (Settings -> Network, 70%) share one unverified claim (no real adapter). Verify together.
* F-STORE-04 (full install to disk, partial 60%) is covered by F-INST-01 (`iyakki install`) and `iyakki-usb --install`; its note says it works. Merge into F-INST-01.
* F-WEB-07 (terminal over WebSocket) says the pixel-streamed CLI tile covers it; deprecate.
* F-BROW-16 and F-BROW-27 (video in pages) overlap; F-BROW-04/43 already plays VP9/Opus on the image.
* F-APPS-04/F-APPS-05 point to F-APPSTORE-06/09/10/02/03 for what exists; merge or mark done with the remaining items split out.
* F-BROW-05 and F-BROW-29 both list SVG and images.

**Marked partial/planned but the notes show it is complete or stale**

* F-BROW-07 (cookies): note says "nothing is saved to disk yet" but F-BROW-31 says persistent cookies are kept in the vault. Mark done and fix the note.
* F-BROW-14 (Settings -> Browser): note says pages do not use style sheets; F-BROW-06 (85%) does. Mark done.
* F-BROW-06: note lists floats, absolute positioning and `::before` as "Not yet"; F-BROW-42 says they are done. Fix the note; progress can rise.
* F-BROW-01: note says "Missing: pictures, forms" which exist (F-BROW-13, 05). Only find-in-page and anchors remain.
* F-BROW-42: progress 15% but the Done text is much longer than the Not-done text; recompute or split into two rows.
* F-APPS-40, F-APPS-41, F-APPS-43: notes name no missing work (85/85/80%). Consider done.
* F-IDE-07: only a style remark in Notes; consider done after one visual check.
* F-SYS-01 (85%): remaining items are enhancements; the tile works. Consider done and open a row for the extras.
* F-EDIT-15: title mixes done (inline completion) and not done (language server). Split.
* F-DISP-02: mixes done (UI size) and planned (resolution). Split, the resolution half depends on F-SHELL-10.

**Marked done but the notes admit gaps (re-check before calling the alpha complete)**

* F-APPSTORE-19 (add a store from a portal link): "The official address is not set yet (F-APPSTORE-14); never run against a real portal."
* F-AI-08 (exact token counts): "Not tried against the live services (needs keys, F-AI-02)."
* F-CLI-02 (install CLI tools): "Tested against fake vendor servers ... not by downloading the real tools"; three tools have no checksum.
* F-DASH-02, F-DASH-03, F-DASH-05: live tests were not run; templates not run against real exporters.
* F-DATA-05 (host files): real drag-and-drop untested. F-BOOT-06 (touch): not tried on a touch monitor. F-ASSIST-12 (screen_capture): not tried on the framebuffer.
* F-SCAN-01: announcement queries not tried against real cameras or TVs. F-JOBS-01/03: restart and real-model long-job runs not done. F-CHAT-17: Stooq endpoint was down.
* Bulk: 143 partial rows and many done rows have notes with "never run"/"scripted models only"; they are in class B, not a row-status problem.

## E. Manual verification checklist by environment

Record PASS/FAIL and the date in `ALPHA-GATE.md` (which already tracks bare-metal, Mac, window, Ollama and provider rows 16-24). Tick each line as it is done.

### Mac mini 2014 (Intel) / Apple-silicon Mac

- [ ] Write the install stick with `iyakki-usb` on the Mac (F-PKG-06): menu lists drives, refuses the system disk, typed confirmation, read-back verify passes.
- [ ] Boot the stick (hold Option). Boot screen with galaxy + progress + `L` log shows (F-BOOT-13); sign-in screen appears; `/etc/iyakki-release` / SYSTEM About shows 0.1.0-alpha.1 (F-REL-01).
- [ ] Use Ethernet (built-in Wi-Fi has no open driver). Measure responsiveness: pointer, typing, window drag feel smooth (F-SHELL-13).
- [ ] SYSTEM -> Hardware: keyboard, trackpad/mouse, sensors listed. STORE -> Installed -> Drivers: install a firmware pack from a USB folder; reboot; confirm the symlink and `modprobe` result (F-DRIVERS-03, F-DRIVERS-07).
- [ ] FaceTime camera (F-DRIVERS-05): supply `AppleCameraInterface`, accept licence, check the extractor accepts it (its checksum table is unverified).
- [ ] Apple-silicon Mac: `./run.sh vm-arm` / UTM boots the aarch64 image to the sign-in screen (F-BOOT-08). `./run.sh window` natively (F-SHELL-20): app opens, finds a local Ollama.

Rows closed by this environment: F-SHELL-13, F-BOOT-13, F-BOOT-08, F-PKG-06, F-DRIVERS-03, F-DRIVERS-05, F-DRIVERS-07

### Bare-metal PC (BIOS and UEFI, Secure Boot OFF)

- [ ] Follow docs/BOOT-TEST.md on at least one BIOS and one UEFI machine; run `iyakki hwreport` and send it (F-BOOT-01).
- [ ] Install to disk from the booted stick: `iyakki install` / Settings -> Install to disk; typed confirmation; reboot into the disk; data survives a second reboot (F-INST-01, F-STORE-04).
- [ ] Check `/dev/zram0` is swap and, with `iyakki.swapfile=2G`, the swap file exists (F-ZRAM-01/02).
- [ ] SSH diagnostics: build with `IYAKKI_SSH_AUTHORIZED_KEYS`, boot entry `iyakki.ssh=1`, key login works, address on sign-in screen (F-SYS-03).
- [ ] Bare-metal cluster: Settings -> Dev cluster -> Set up a local cluster (needs 1 GB+ RAM): download, SHA-256 check, node Ready, health row in SYSTEM, reboot with Autostart (F-K8S-13, F-K8S-14, F-CLI-03).
- [ ] Minimal systemd distro (not Alpine/Debian/Ubuntu/Fedora/Arch containers already pass): run `dist/test-bases.sh` and the unit file there (F-PKG-14).

Rows closed by this environment: F-HW-02, F-PKG-14, F-K8S-13, F-K8S-14, F-CLI-03, F-STORE-04, F-BOOT-01, F-SYS-03, F-ZRAM-01, F-ZRAM-02, F-INST-01

### Window mode on Linux / macOS / Windows

- [ ] `./run.sh window` (or the tarball `run-window.sh`): create the admin account, open CHAT, EDIT, SYSTEM; title shows the version; `iyakki-desktop --version` (Alpha gate rows 17-18).
- [ ] Open IDE, DASH, FLOWS, STORE, BROWSER at a normal window size and look: no overflow, text readable (F-IDE-07, F-DASH-06, F-FLOW-09, F-IDE-09).
- [ ] Drag a file in from the file manager and use Save to host (F-DATA-05, done-with-gap).
- [ ] Settings -> Voice -> Browser voice helper: button opens the default browser, page says connected, test speech is heard, Disconnect works (F-VOICE-06).
- [ ] Repeat the above on Windows and macOS builds; log anything that fails to open or crashes (F-SHELL-09).

Rows closed by this environment: F-SHELL-09, F-IDE-07, F-DASH-06, F-FLOW-09, F-IDE-09, F-VOICE-06

### Real k3s cluster (a throwaway one)

- [ ] Start k3s on a test box. IDE -> K8S: chip says reachable; Cluster... chooser picks it (F-IDE-02, F-K8S-07).
- [ ] Deploy... a hello-world: build, load (in-cluster registry via Registry...), apply, verify; address opens in the Browser tab (F-K8S-02, F-K8S-06).
- [ ] Run a test target (`deploy_run_job`) and a failing pod to see `bug_report` evidence folders (F-K8S-10, F-BUG-01).
- [ ] Tear down... Stop, Start, Remove all with the typed name; refuses `default` and unlabelled namespaces (F-K8S-08).
- [ ] Settings -> Dev cluster: Test connection, Hide, Disconnect, registry A/B/C (F-K8S-12).
- [ ] Platform services: deploy Gitea and the private store; open, stop/start, backup, remove (F-K8S-16/17/18, F-APPS-42).
- [ ] FLOWS: run the build-test-deploy template and a sandbox Job (F-FLOW-04, F-FLOW-05).

Rows closed by this environment: F-BUG-01, F-IDE-02, F-K8S-02, F-K8S-06, F-K8S-07, F-K8S-08, F-K8S-10, F-K8S-12, F-K8S-16, F-K8S-17, F-K8S-18, F-APPS-42, F-FLOW-04, F-FLOW-05, F-K8S-13, F-K8S-14, F-CLI-03

### Managed cloud / in-pod

- [ ] Run the container (`dist/k3s-web.yaml`) on a managed cluster (EKS/GKE); sign in via the Service (F-WEB-06).
- [ ] Inside a pod: In-cluster profile appears, a cluster-admin service account is read-only, a vault kubeconfig works and is removed from /dev/shm afterwards (F-K8S-15).

Rows closed by this environment: F-K8S-15, F-WEB-06

### Ollama (qwen3:8b and qwen2.5-coder:14b)

- [ ] Chat: ask for a file edit, a web lookup and a multi-tool task; no refusal loops, no "please run this yourself", answer streams (F-ASSIST-10/11/14/19/28).
- [ ] Paste a fake API key in chat: the guard blocks it; ask the assistant to need a secret: masked form appears (F-VAULT-05).
- [ ] IDE workspace: ask for a multi-step job; the assistant adds board rows and does not mark done without a verified check (F-ASSIST-15/17/20/27).
- [ ] Autopilot a real 10-16 step build-and-deploy goal: finishes or parks rows with a diagnosis, no tear-down question loop (F-ASSIST-18/21/23/25/26/29, F-IDE-08).
- [ ] Stall test: use a thinking model with a tiny silence limit; it retries once with thinking off (F-ASSIST-22).
- [ ] DASH: ask the assistant to build a panel and explain a spike (F-DASH-08). TALES: write a kids story with qwen3:8b, check the scene JSON drawings (F-TALES-16). FLOWS: break a node and watch the healer (F-FLOW-06).

Rows closed by this environment: F-ASSIST-10, F-ASSIST-11, F-ASSIST-14, F-ASSIST-15, F-ASSIST-17, F-ASSIST-20, F-ASSIST-23, F-ASSIST-26, F-ASSIST-27, F-ASSIST-28, F-ASSIST-29, F-IDE-08, F-ASSIST-18, F-ASSIST-19, F-DASH-08, F-TALES-16, F-FLOW-06

### DuckDB

- [ ] Install DuckDB from the CLI tile (real download, no published checksum), then `IYAKKI_LIVE_DUCKDB=... cargo test -p iyakki-agent dash::live_tests::duckdb_live -- --ignored` (F-DASH-04). Same for IYAKKI_LIVE_PROM / _ES / _CLICKHOUSE (done rows F-DASH-02/03).

Rows closed by this environment: F-DASH-04

### Claude Code and GitHub Copilot CLI (real, signed in)

- [ ] Install through Settings -> Agents / CLI tile (real npm install); sign in with the vendor command in the terminal; Detect shows ready (F-EXTA-04, F-EXTA-06).
- [ ] Chat with Claude Code as the agent: streaming text, tool blocks, `--resume` on the second message, model list, tokens/cost in USAGE (F-EXTA-01, F-EXTA-03).
- [ ] Confirm the policy bridge: an edit outside the workspace, raw `kubectl`, `sudo` are refused; approvals show the agent name; check that `_meta` tool-use ids are sent (F-EXTA-02).
- [ ] Run Autopilot with Claude Code for 3 steps; cost cap and pause work (F-EXTA-05). Start `job_start` for a 40 s command and check it does not poll (F-JOBS-04). Copilot: confirm advisor-only, check JSONL event shape.
- [ ] TALES with Claude Code as writer and as drawer of pictures (F-TALES-11, F-TALES-15).
- [ ] IDE parallel tasks with a real Claude Code: 2 tasks, review diff, Merge (F-IDE-10).

Rows closed by this environment: F-EXTA-01, F-EXTA-02, F-EXTA-04, F-EXTA-06, F-JOBS-04, F-TALES-11, F-TALES-15

### Real Wi-Fi adapter and access point

- [ ] Settings -> Network: `w` scans; join an open, a WPA2 and a hidden network; reboot, it reconnects; forget it (F-NET-02, F-NET-07). WPA3 if available.

Rows closed by this environment: F-NET-02, F-NET-07

### Microphone, speaker, camera

- [ ] Settings -> Voice -> test speaker; hear it. Read a short kids story in TALES; pause/seek (F-TALES-02, F-VOICE-05; try espeak-ng, piper if installed).
- [ ] Dictate with a real microphone, then unplug it mid-way; error is plain (F-VOICE-04, F-HW-02).
- [ ] MEDIA: play an MP3 in IYAKKI and an MP4 with mpv; the sample video on the web image (sound muted in guest mixer: unmute) (F-MEDIA-01, F-MEDIA-07).
- [ ] Camera capture with a real webcam (F-HW-02).

Rows closed by this environment: F-HW-02, F-VOICE-04, F-VOICE-05, F-TALES-02, F-MEDIA-01

### `web` image (WPE helper) on QEMU and then bare metal

- [ ] Run `dist/buildroot/qemu-web-test.sh` and `qemu-media-test.py`: google.com, Wikipedia, YouTube render and play (F-BROW-04, F-BROW-43, F-MEDIA-07).
- [ ] On bare metal with a real GPU: same sites; log frame rate and web-process memory (F-BROW-04).
- [ ] Assistant browser automation on the image: the WPE helper was only compiled, run `browser_open/snapshot/click` (F-BROW-44).

Rows closed by this environment: F-BROW-04, F-BROW-43, F-BROW-44, F-MEDIA-07

### A real image model (e.g. flux via an OpenAI-compatible server)

- [ ] TALES with a picture model configured: each scene is a real image; kill the server mid-story to see the SVG fall-back (F-TALES-13). Agent returns PNG/JPEG: format detection and caps (F-TALES-18).

Rows closed by this environment: F-TALES-13, F-TALES-18

### Real IP camera on the LAN

- [ ] Discovery finds the camera; confirm, credentials land in the vault (F-CAM-01).

Rows closed by this environment: F-CAM-01

### Real remote host (internet)

- [ ] Queue 3 direct media links from a real server incl. a redirect and a too-large file; failure wording is right (F-MEDIA-06).

Rows closed by this environment: F-MEDIA-06

### Real provider keys (OpenAI, Anthropic, Gemini)

- [ ] Settings -> Models: add each key; chat with one tool call; token counts match the provider's usage (F-AI-02, and done row F-AI-08). Gemini streaming is not built.

Rows closed by this environment: F-AI-02

### Owner build machine

- [ ] Rebuild the images, then `./run.sh alpha-check` (not `--no-tests`) and `./run.sh release --dry-run`, then `./run.sh release`; verify `/etc/iyakki-release` inside a running image (F-REL-01, F-REL-05).
- [ ] Back up `~/.config/iyakki/release.key` offline; publish the public key.

Rows closed by this environment: F-REL-01, F-REL-05

### Alpha-gate rows that are only a command

- [ ] `./run.sh alpha-check` without `--no-tests` (gate row 3); `cargo test -p iyakki-core --test feature_matrix` after the status corrections above.

## F. Proposed order for class A: three waves of parallel agents

Rule: one agent owns each of `chat.rs`/`story.rs`, `ide*.rs`/`board.rs`/`edit.rs`, `store*.rs`, `settings*.rs`, `media.rs`, `browser.rs`. Every agent edits only its own rows in `FEATURES.md` at the end, in one small commit, to avoid matrix conflicts; run `cargo test -p iyakki-core --test feature_matrix` before merging. Within a wave the agents are independent.

### Wave 1: independent crates and split browser files (start now)

* **Agent 1a, status clean-up (FEATURES.md only, XS)**: F-APPS-40, F-APPS-41, F-APPS-43, F-BROW-07, F-BROW-14, F-BROW-01 note, F-BROW-06 note, duplicates and splits above; update the summary line and the matrix test.
* **Agent 1b, macOS window build (L)**: F-SHELL-20: cfg fences in fb.rs, input.rs, lib.rs, core main.rs/boot.rs only; no app files. Most valuable: the owner tests on a Mac.
* **Agent 1c, assistant security and agent backend (S x4)**: F-VAULT-05, F-EXTA-03, F-EXTA-05, F-MEDIA-05 (mediadl.rs, credguard.rs, extagent, autopilot.rs).
* **Agent 1d, browser engine files (M x5)**: F-BROW-05, F-BROW-29, F-BROW-36, F-BROW-24, F-BROW-41 (browser_images.rs, browser_logins.rs, browser_devtools.rs, iyakki-web). Not browser.rs.
* **Agent 1e, editor crate (S x2)**: F-EDIT-14, F-EDIT-17 (iyakki-edit, iyakki-ui, iyakki-gui).

### Wave 2: the big shared UI files, one owner each (after wave 1 merges)

* **Agent 2a, chat/story owner**: F-DATA-03, F-ASSIST-24, F-ASSIST-21, F-TALES-09 (chat.rs, chat_view.rs, story.rs, boardtrack.rs, autopilot.rs).
* **Agent 2b, IDE owner**: F-IDE-01, F-IDE-05, F-IDE-06, F-IDE-10, F-ASSIST-25, F-EDIT-16 (ide*.rs, board.rs, edit.rs).
* **Agent 2c, browser.rs owner**: F-BROW-22, F-BROW-23, F-BROW-25, F-BROW-34, F-BROW-37, F-BROW-39, F-BROW-01 (browser*.rs).
* **Agent 2d, media owner**: F-MEDIA-03, F-MEDIA-04 (media.rs).

### Wave 3: store and settings (after wave 2)

* **Agent 3a, store owner**: F-APPS-02, F-APPS-03, F-APPSTORE-10, F-FLOW-11 (store*.rs, skills.rs, wtool.rs, pkg.rs).
* **Agent 3b, settings owner**: F-DISP-02, F-ASSIST-22 (settings*.rs, uisize.rs, per-page render tests at 800x480 and 1024x600).

After wave 3: re-run `./run.sh alpha-check`, then the class-B checklist.
