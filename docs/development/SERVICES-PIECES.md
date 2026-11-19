# The Services read facets: who owns which members (split-a and split-f, 2026-11-17)

Plan: `docs/development/SPLIT-GROUPS.md`, section "The Services read facets: plan v2". Two lanes work on DISJOINT members so the commits do not touch the
same lines. Merge `release/alpha.6` often; one commit per piece; `facet_defaults` and `boundary` green after each.

| Piece | Members | Owner |
|---|---|---|
| 1, 2, 3 | facet accessors, shared-typed reads, `iyakki-compose` | split-f (done) |
| 4 | `k3sdev`, `cluster`, `devwatch`, `ollama`, `seaweedfs`, `rustfs`, `clis` | split-a |
| 5 | `browser`, `screen`, `camera` | split-a |
| 6 | `jobs`, `runs`, `sessions`, `skills`, `mcp`, `packages`, `devtools`, `radio` | split-a |
| 10 | the one-use tail: lemonade, wake, fleet, cloud stream, sftp, system sounds, and every member not named in another row | split-a |
| 7 | `host_tts`, `ext_status`, `ext_sessions`, `client_for`, `models`, `image_models`, `lemonade_*`, `picture_choices`, `usage` (`AiRead`) | split-f |
| 8 | `user_*`, `system_set`, `secret_requests`, `answer_secret_request`, `cancel_secret_request` (`VaultRead`) | split-f |
| 3b | `dir` on non-`cx` receivers | split-f |
| 9 | `clone`, the 11 `Services` type names, the dependency removal | split-f |

Done by split F: `AppCx` accessors for `settings`, `inbox`, `calendar`, `study`, `secrets`, `kb`, `voice_settings`, `sound_settings`, `agents_cfg`, `b0`, `b3`, `b4`, `b5`, the voicedesk, extagent, tasks, flows facets and `git_kit`;
`iyakki-compose` and its real-wiring guard (`crates/iyakki-compose/tests/real_wiring.rs`); `iyakki-shell` has `iyakki-agent` as a dev-dependency only.
When a piece needs a file move, run `cargo test -p iyakki-core --test boundary` on `rcargo` too (BOUNDARY.md rows follow the file).

Done by split G (continuing split F): the model and speech group (`host_tts`, `ext_sessions`, `client_for`, `image_models`, `picture_choices`, `usage`), the vault group, `secret_requests`, `notify`, the `dir` reads and the `ShellEnv` accessor block. Open for split G: the closure captures of `svc`, the type removals and the `testing` feature, all of which wait for the last members of split A (see the end of `SPLIT-GROUPS.md`).
Rules: the lane that owns a member adds its `AppCx` method (or the facet method), then rewrites the readers with `dist/split/rewrite_services.py`.
A member that is not in this table is split-a's (piece 10) unless split-f has already started it; ask first. Neither lane edits the other's `AppCx`
methods; add yours next to your own group in `appcx` and keep the new methods in their own block so merges stay clean.
