# Chat and IDE ports: what is done and what remains for the tile crates (alpha.10, lane P4)

**For:** the owner and the next lane. **You will get:** the state of the Chat and IDE ports, the rule for growing them, the sizes of what is left, and the order I would do it in.

## What is written

* `crates/iyakki-apps/src/apps/chat_port.rs`: the trait `ChatPort`, the real port `CxChat` (a cheap `Copy` view over `AppCx`) and the test fake `FakeChatPort` (manual clock, in-memory audit list; the saved-chat store, vault, external-agent status and secret requests are given with `with_` builders and a missing one stops the test with a sentence, it never falls back to the real thing).
* `crates/iyakki-apps/src/apps/ide_port.rs`: the trait `IdePort`, the real port `CxIde` and the test fake `FakeIdePort` (same rules).
* The files that reach the person, policy, role, data folder, audit log, core, settings, saved chats, session folders, vault, voice settings, external agents, browser state, Git kit, CLI tools, store facets, task manager, run store, knowledge base and quick upstream now do it through the port. A raw `cx.session` or `cx.audit` no longer appears in non-test chat or IDE code (one `cx.session.user` in the chat's own sign-out guard and the `cx.audit` of the Chat approvals are the only reads left; see the table).
* Tests: `chat_port_tests.rs` (8) and `ide_port_tests.rs` (7) test both ports (real against the test context, fake with a manual clock), run a chat tool file (`chat_monitor::entries`) on the fake port with no shell context, and prove the fakes never fall back to the real thing. The existing Chat and IDE tests (about 740) run unchanged against the real port.

## The rule for growing the ports

A new need of a Chat or IDE file becomes a method on the trait (real port answers from the context, fake from memory), never a new `cx.` in the file. The inventory (`python3 dist/scripts/app-inventory.py`) counts the uses; the `uses` column of both rows fell from 160 to 12 (Chat) and from 200 to about 36 (IDE) in this lane.

## What is left for the tile crates (sizes are lines of non-test code; counts are the mentions of `cx` that remain in non-test code)

| Step | Chat | IDE | Why it is separate |
|---|---|---|---|
| 1. Signatures: `&AppCx` becomes `&dyn ChatPort` / `&dyn IdePort` in the functions that only read the port | about 350 `cx` mentions in 21 files; `chat.rs` 113, `chat_browser_data.rs` 32, `chat_view.rs` 29 | about 750 mentions in 16 files; `ide.rs` 79, `ide_git.rs` 30, `ide_browser.rs` 29, `ide_tile.rs` 28 | Mechanical but wide: each function that hands `cx` on to `EditApp::new(cx)`, `core_run::run_external(cx, ..)`, `ChatApp::resumed(cx)` and the embedded Browser, Files, Terminal and Editor views needs those callees on the port first. About 40 places clone `cx` to move it into a spawned run; they need an `Arc<dyn ChatPort>` (a `Send + Sync` port) and the run closures changed once. |
| 2. Remaining port methods | `cx.services.*` calls (`user_tools`, `reading_synth`, `note_fleet_press`, `effective_tts`, `delete_session`: 5), `cx.agent.*` (3), `cx.host`, `cx.chat`, `cx.os.disks`, `cx.mcp_servers`, `cx.pendingapi`, `cx.host_tts`, `cx.extagent_facet`, `cx.tasks_facet`, `cx.kb_source`, `cx.browser_consent`, `cx.approval` | `cx.services.*` (`dev_deploy`, `router_plan_rows`, `platform_real`, `git_keys`, `fleet_attach_workspace`: 11), `cx.agent.*` (21 helper calls: `workspace_locks`, `context_intent`, `clusters_phrase_for`, `wsexec_git_available`, `projectstate_is_known_stack`, `pkg_newer`, `gitkeys_valid_host_key`, ...), `cx.os.fleet`, `cx.platform_handles`, `cx.host`, `cx.chat`, `cx.mcp_servers` | About 40 more methods in each trait. The `cx.agent.*` helpers are pure functions of the agent crate: they should move behind one `IdeHelpers` trait or into the shared crate, not onto the port. |
| 3. Pure leaf model crates (D-S2-01 option (a): a pure leaf crate re-exported by the old path) | `chat_hostname.rs` (201), `chatpic.rs` (714), `chat_toolscore_reqs.rs` (141), the message layout in `chat_view.rs` (2,348): about 3,400 lines, no `cx` at all in the first three | `ide_layout.rs` (331), `ide_tasks.rs` (894), `ide_bottom.rs` (997), `ide_deploy.rs` (486), `ide_git_connect.rs` (847), `ide_router.rs` (60): about 3,600 lines, no `cx` at all | These files already do not name `AppCx`; they can leave first, as `iyakki-chat-model` and `iyakki-ide-model`, with their tests, and be re-exported at the old path. Their dependencies on `iyakki_shared` types need the allow-list lines the owner approves (D-S2-01). |
| 4. Tile crates `iyakki-tile-chat` and `iyakki-tile-ide` | `chat.rs` (5,800 lines, a quarter of it inline tests) and the files of step 1 | `ide.rs` (3,543), `ide_k8s.rs` (1,538), `ide_notes.rs` (1,025), `ide_git.rs` (960), `ide_browser.rs` (921) | A tile crate may depend only on `uikit`, `tile-api` and approved allow lines. Chat and the IDE embed the Editor, Browser, Files and Terminal views and the chat embeds the Jobs list (already a tile crate); until the SDK views cover each of those, the tile crates stay in `iyakki-apps` behind the port. |

## Order I would take

1. Step 3 for the files with no `cx` (small, safe, shrinks `iyakki-apps` by about 7,000 lines, no behaviour change).
2. Step 2 for the facets, a commit per port method group, with a fake test each.
3. Step 1 one file group at a time, smallest first, starting with the chat tool files (already on `&dyn ChatPort`).
4. Step 4 only after the Editor, Browser and Terminal embed views exist as SDK views.

## Known test flake seen on the Macs (not from this lane)

`apps::chat::tests::a_picture_on_top_of_the_clipboard_history_is_kept_in_uploads_and_goes_with_the_next_message_and_text_pastes_as_before` fails when run alone on a Mac (it fails the same way on the commit before this lane) because it reads the process-wide clipboard other tests also write; it passes in the full run. Two IDE tests (`creating_a_repository_is_reviewed_first_...`, `running_tasks_are_agents_on_the_board_...`) failed once in a 7-minute loaded run and pass alone.
