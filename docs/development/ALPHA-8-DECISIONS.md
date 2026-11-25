# Alpha.8 decisions taken by the model on the owner's delegation (2026-11-21)

**For:** the owner. **You will get:** every decision that was waiting for you, what was decided and why, and what is parked. You can overrule any line; say which and it is changed.

Rule given by the owner: "You can decide on those where you are waiting for my decision. Judge if very useful then add them, otherwise add to park." Parked means kept on the list, not deprecated.

## Adopted as implemented (useful, small risk)

| Decision | Outcome |
|---|---|
| D-ARCH-07-TYPES (type-only re-export of device types in `iyakki-core-api`) | Adopted. Apps never name the device crates. |
| D-ARCH-07-DISKTOOLS (`installer::DiskTools`, `StartupDisk::firmware_present` and `own_entry_first`, `Drivers::pack_files` and `remove_pack`) | Adopted. Core-api additions are in `core-surface.txt`. |
| F-ARCH-07, the 25 reader pairs | Adopted: the owner crates (storage, devctl, usb, fw, netscan, linuxinit, shared, stage1, ui-bin, apps input) stay the named single readers. The row's goal now reads "reader pairs only in named owner crates". The restructure that breaks the dependency cycle is parked. |
| D-SPLIT-07-RATE (200,000 calls a second for the first-party UI program) | Adopted: the rate is not a defence on this socket; layers 1 to 4 are. Per-method limits parked. |
| D-SPLIT-03-POLICY and name-based identity | Adopted for now; the stronger binding is scheduled (see below). |
| D-SPLIT-03-TILES (Store tile apps served over the UI socket without the display-list pair) | Adopted. The approved display-list pair stays available and unused. |
| Flows tile plain files (`FailBook`, live picture, file import) | Adopted as SDK migration work, scheduled with Flows in alpha.9. |
| D-SDK-34-01 (ABI 3 only for tiles that declare it) and D-SDK-34-03 option (a) (`dock.open` for allowed apps, audited) | Adopted. A `dock` capability (option b) is parked. |
| SDK version switch in Settings, Appearance | Adopted. Separate launcher tiles with their own app ids are parked. |
| Flows: a hook's secret does not cross the socket (the panel points to the vault); `flows:hooks` not in the UI program's default scopes; `FlowsMeta::audit` under the account | Adopted. |
| `Applied.reveal` (generated admin password) does not cross the socket | Adopted; the deploy dialog points to the vault. |
| Tile crate allow lines: `iyakki-tile-hostfiles` to `iyakki-hostfiles`; `iyakki-tile-disks` to `iyakki-disks-model`; `iyakki-tile-learn` to `iyakki-study-model`; `iyakki-tile-dash` to `iyakki-dash` and `iyakki-edit`; `iyakki-tile-k8s` to `iyakki-k8s-model`, `iyakki-cluster-model` and `iyakki-sec`; `iyakki-tile-devtools` to `iyakki-devtools-engine` and `iyakki-qr` | Adopted. Replacing `iyakki-sec` in the K8s tile by a trait is parked. |
| New base crates `iyakki-hostfiles`, `iyakki-site-apps`, `iyakki-devtools-engine`, `iyakki-board-model`, `iyakki-disks-model`, `iyakki-k8s-model`, `iyakki-cluster-model`, `iyakki-study-model`, and the tile crate name `iyakki-tile-runlog` | Adopted. |
| Plain mirrors in `iyakki-tile-api` as the pattern for apps that cannot use a pure leaf crate (D-S1-E4-01, D-S1-E5-01, D-S1-E6-01, D-S1-E6-02); Chat depending on a tile crate for the Jobs list | Adopted. |
| Word-aware `Services` ratchet (D-S1-E4-02) | Adopted. |
| D-E1 evidence roles (`admin` and `person` accepted, 10 percent of those passes sampled for the owner) | Adopted. |
| Human evidence 10 percent sample reviewed by the owner | Adopted. |
| Earlier core-api review items (Cues serde, `DiskEngine`, `AudioRoute`, `Drivers`, `StartupDisk`, `BtAudio`, `ScreenFacts`, capabilities `system.disk.mount`, `system.disk.partition`, `system.disk.unlock`, `system.link.set`, `system.startup.set`, `files.client`, the `client.ui` row) | Adopted as implemented; they are the interface the split needs. |
| F-BROX-08 and F-BROW-100 "closed without new code"; F-QA-11 row kinds for espflash and 7zip; the Unicode-3.0 asset row; `api:flows` does not cover hook management (`flows:hooks` stays separate) | Adopted. |
| D-ACCEL-VIDEO-DEFAULT | Already confirmed by the owner. |

## Added to the work (very useful)

1. **Bind the acting person to the socket login** (closes D-SEC1-SHARED and D-SEC1-APPROVED, and removes the "a name is believed" limit), with a core-issued one-time approval ticket for the platform calls that carry `approved`. Beta gate 6. Lane SEC3 in alpha.8.
2. **Split the eight pilot rows** (F-CORE-20, F-CORE-29, F-SDK-189, F-SDK-191, F-SDK-212, F-SDK-213, F-SDK-214, F-SKIN-07) into a visible sub-row and an API sub-row so the autotest can raise them. Splitting adds sub-rows, changes no behaviour, and never lowers a number. Lane SPLIT1 in alpha.8, then more rows by the same rule as the runner's results show.
3. **Machine kind picked once in the Manual tests tab** (a "Mac mini" without a year is not guessed). Small. Lane E1b in alpha.8.
4. **K8s long-lived port-forward as its own facet** (option A) and exec in the Terminal view: alpha.9, with the medium apps.
5. **Edit and Cli browser tiles** (wasm32 build, registered in `tiles::builtin()`): alpha.8, needs the AMD host after the gate.

## Parked (not deprecated)

* QEMU evidence counting as real for software-only rows (rung 100). Reason: "we should not make a false claim"; revisit at the beta gate once machine runs exist. The switch stays off.
* Real push service for Browser Web Push (D-BROWSERPUSH-SERVICE). When built: self-hosted UnifiedPush; until then none (the default).
* F-STORE-05 stage1 dependency on `iyakki-volcrypt` and the stage1 console adapter (boot code, needs a QEMU boot test): after Test 75.
* Marking one account as "the owner" in the device (the device cannot tell the product owner from another administrator): parked; admin and person roles are enough with sampling.
* Typed core-api trait per tile for the device types (the re-export is the final form), a `dock` capability, separate SDK launcher tiles, per-method rate limits, restructure of the owner-crate dependency cycle.
* The Opus encoder (F-CLIENT-03 stays on the per-person mixer).
* GitHub as primary repository: after the beta gate 8 checks.
* DHCP reservations for the build machines: an action in the owner's router, not a code decision.

## D-N2-01 (decided by the model, 2026-11-21): Inbox, Lab and System extraction pattern

Inbox and Lab draw from domain types that are pure data and rules (`iyakki_shared::inbox`, `iyakki_shared::lab`): use leaf model crates re-exported by the old path (option a). System touches live machine types (`iyakki_core_api::sysinfo`, `sysprocs`, `k3sdev`, `gpu`, `OwnedProc`): use plain mirrors in `iyakki-tile-api` (option b), like Rag. Tales (one 10,466-line file, about 190 shell-context uses) gets its port (`TalesPort`, about 40 methods) before any crate. Owner can overrule.

## D-P1-01 (decided by the model, 2026-11-21): Tales on a port first, the crate after a model crate

TALES was one file of 10,466 lines with about 190 uses of the shell context. Alpha.9 lane P1 split it into the module `apps/story/` (nine files by job, plus the tests; no behaviour change, the 97 old tests unchanged) and moved every use behind `TalesPort` (45 methods: person and account, library and PDF, chat and picture models, agents, speech, clock) and `TalesWorker` (what the threads the tile starts may ask: Lemonade, the picture request, an agent run, a link fetch). The real port is `impl TalesPort for AppCx` in `story_port.rs`, so the old tests that hand the tile an `AppCx` still run through it; nine new tests run the tile on an in-memory fake with a manual clock. The tile joins `migrated_tiles.list` (ten files).

The crate extraction (`iyakki-tile-tales`) is not done, and the reason is exact: the tile's data types are `iyakki-shared` types with logic (`story`, `storyillus`, `scene`, `piclog`, `hostvoice`, `voice`, `chat`, `hostengines`, `lemonade`, `providers`), `iyakki-agent-api` types (`ExtRunSpec`, `LinkedPicture`, `DirectVoice`, `HostTtsApi`) and the shared model picker's `ModelList`. Default decided: option (a) of D-S2-01 for the pure ones (a leaf crate `iyakki-story-model` holding `story`, `storyillus`, `piclog` and the scene types, re-exported by `iyakki_shared` under the old paths, one allow line), plain mirrors in `iyakki-tile-api` for the agent-api and picker types (D-S1-E4-01 option (b)). OWNER: say whether to go on that way; it is two crates and about 25 mirrored types and needs its own round.

## KV260 and Raspberry Pi 5 (decided by the model on the owner's delegation, 2026-11-21)

* **KV260 boot: extlinux** (the board's own U-Boot looks for `extlinux/extlinux.conf` on the first bootable FAT partition, then `boot.scr`, then EFI); EFI is not needed. Adopted.
* **First KV260 test: serial first, then DisplayPort** (the checklist asks for both in that order). Adopted.
* **Release-profile (fat LTO) rebuild before Monday: parked.** The fast profile is enough for a first boot; rebuild after the first-boot report. 
* **aarch64 web variant for the KV260: parked.**
* **Pi packs and the signed downloads manifest:** the two Pi firmware packs stay out of the manifest (pinned by size and SHA-256 in `legal/downloads.txt`, shown and accepted at the person's click). Parked.
* **Pi 3 lite:** after the Pi 5 first-boot report.

## D-SESSION-END (decided by the model on the owner's delegation, 2026-11-24): what ends when a person signs out

Found by the owner: after sign-out the radio kept playing. Cause: the machine has one mixer with no per-person sources; `soundowner::signed_out` only removes the person from the list of who owns the sound (so a remote client cannot take it) and nothing stops the playback. Sign-out in `Shell::sign_out` is a hand-written list of per-feature stops; sound, capture and several session processes are not in it.

**Rule (adopted):**
* **Sign-out ends the person's interactive session resources:** radio, media player and queue, text-to-speech and story/voice readers, dictation and voice sessions, wake listening, microphone, camera and screen capture and sharing, live previews, terminal and pod sessions, port-forwards, sandboxed Linux windows, IDE and Lab jobs started by the person, and the sound of a client session. Each is stopped by one registered end-of-session hook, not by a new line in `sign_out`.
* **Switching person:** when another person becomes the owner of the sound, the previous person's sources stop.
* **Lock or idle:** playback continues (as on every desktop OS); microphone, camera and screen capture stop.
* **Continue on purpose:** scheduled flows, alarms and other services the person set up to run on their own continue; each is listed in one place and says so in its row.
* **Design:** one registry of end-of-session hooks in the core (`SessionEnd`), every owner of a long-lived per-person resource registers one, `sign_out` calls the registry once and audits what it stopped; a conformance test lists every registered kind and proves that a sign-out stops it; a ratchet fails a new per-person player, capture or session thread that is not registered.
* **Not decided here (parked):** a Settings switch "keep playing after sign-out" (default off): not needed until someone asks.

## Lifecycle audit decisions (LC1 questions, decided by the model on the owner's delegation, 2026-11-24)

* **P-01, radio after a full pass of failing stations:** it stops with a plain message ("no network") and starts again by itself when the link returns (the link event now needs a default route through an up interface); slow retries meanwhile (1 minute, then 5, then 15). Adopted, row F-LIFE-11.
* **P-02, cloud client past 120 seconds:** keeps retrying with back-off and shows a notice; it does not stay closed. Internet sharing resumes when the uplink returns. Adopted, row F-LIFE-17.
* **P-05, Bluetooth headset leaves:** playback pauses (as on macOS and Windows) instead of moving to the room speakers; a setting to move it is parked. Adopted, row F-LIFE-14.
* **P-09, shell crash:** a person who is not in developer mode gets a plain notice after the restart ("the screen restarted"); session restore stays developer-only. Adopted, row F-LIFE-16.
* **Lock-time and suspend-time capture stop (KI-1022):** part of the SO1 registry (D-SESSION-END): a lock-time kind and an on-suspend kind.
* **KI numbers:** KI-LC-A to H are KI-1022 to KI-1029.

## LC2 core-api additions (lane LC2, for owner review)

* `SessionResume::take_unexpected_end(&self) -> bool` (default false), F-LIFE-16: true once when the shell ended while somebody was signed in (a marker with no name, removed at sign-out). Listed in `core-surface.txt`.

## D-SESSION-END, lane SO2 additions (decided by the model on the owner's delegation, 2026-11-24)

* **Suspend:** a new reason `Why::Suspend` runs the `Capture` and `Sound` hooks before the machine sleeps (`Shell::sleep_now`, which also covers the lid path, and `boot::sleep` for every other way, including the browser client's sleep button). Terminals, forwards and jobs go on through a sleep.
* **Jobs stop at sign-out unless marked:** IDE task runs and lab or chat jobs started by the person stop at sign-out; a job marked with `keep_after_sign_out` (on `job_start`) or the chat tool `job_keep` (both ask) continues. System install jobs (workspace `iyakki-install`) and media downloads continue: they are not sound or capture.
* **Continue on purpose, written reasons:** the domain listener of Settings -> Domains (a service for other people) and the cloud `AnswerPlayer` (a buffer, no device; its program runs on the person's computer). The ratchet has 0 `open` lines.
* **Live previews:** no separate resource: the camera preview is the camera hook, the others hold no process or device.

## D-AP1-01 (alpha.11 AP1, Tales onto a tile crate: steps 1 and 2)
* Question: next pieces of the D-P6-03 remainder.
* Done: `iyakki-story-kit` gains `steer` (the revision messages, `fenced`, picture requests and feedback, `reuse_pictures`, the hint parsers `parse_hints*`, `words_of`, `minutes_in`, `auto_style`, `valid_pic_file`, `valid_var_file`, `MAX_PICTURES`) and `speech` (`tts_status*`, `effective_engine*`, `status_text`, `voice_choices`, `QuickEngine`, `shell_word`, `find_piper_voice`, the two help texts). `iyakki_shared::story` re-exports both modules, so no caller changed. `iyakki-story-kit` now depends on `iyakki-sec` (base). 6 new direct tests. No core-api change, no allow line.
* Still in `iyakki_shared::story`: `StoryStore` (needs a trait over the library files and the policy), `StoryChat`, `Prefs`, `ModelPlan`/`plan_model`, `quick_engines` (names `providers::ProviderCfg`), `follow_agent_run` (needs run-type mirrors). Then the `iyakki-tile-api` mirrors, `story_port.rs` conversions and `git mv apps/story/` into `iyakki-tile-tales`; then Voice. Not started.

## D-AP1-02 (alpha.11 AP1): `iyakki-tile-tales` exists; the library, the reading traits and the run loop moved to leaf crates instead of mirrors
* Question: how does the TALES screen leave `iyakki-apps` (D-P6-03), with the owner's "about 25 plain mirrors in `iyakki-tile-api`"?
* Default implemented (D-S2-01 option (a): a leaf crate re-exported by the old path, wherever the type is plain or a trait over leaf types; fewer copies and no conversion code):
  * `iyakki-story-kit::library`: `StoryStore` (a struct, not a trait: it is the file scope and the policy over `iyakki-sec`, `iyakki-filescope`, `iyakki-schema`), `StoryDoc`, `Prefs`, `StoryChat`, `ModelPlan`, `plan_model`. `resolve_for_write` and `blocking_file_text` moved to `iyakki-filescope`.
  * `iyakki-story-kit::follow`: `follow_run` over a `RunFeed` trait and plain `RunMsg` (3 tests); `iyakki_shared::story::follow_agent_run` is a thin adapter over the agent's run handle. The tile starts an agent through `TalesWorker::start_agent(AgentRunReq) -> Box<dyn RunFeed>`; the shell builds the agent's own request (no tools, one step, 900 s, never auto-approved). This replaces the mirrors `ExtRunSpec`, `RunHandle`, `RunLimits`, `NoTools`.
  * `iyakki-story-model`: the traits `StoryReader`, `NoReader`, `DirectVoice`, the `LinkedPicture` and `StoryPdf` data (`iyakki-agent-api` and `iyakki_shared` re-export them). `iyakki-voice-model`: `HostTtsApi` and `HelperState`. `iyakki-provider-kind`: `ImageParams`, `PullStatus`. So no mirror and no conversion for these.
  * `AgentsCfg` and `ExternalAgent` are not mirrored: the port answers narrow questions (`agent_known`, `agent_label`, `agent_advisor`, `agent_check`, `agent_model_label`, `agent_key_ref`, `agent_entry`, `scene_agent_chat`). `Os` is not mirrored: the port hands a rewording function. `speaks_directly`, `can_save_audio`, the machine's memory and a new story reader are port methods.
  * `iyakki-tile-tales` (dependencies: uikit and the leaf crates in `tile_crate_deps.allow`): `TalesPort` (about 50 methods), `TalesWorker`, `AgentRunReq`, `AgentEntry`, the nine screen files, `Flow`, `Redraw`, a `testing` feature. `apps/story.rs` is the `Deref` adapter (like LAB); the 97 screen tests (`story_tests.rs`) and the perf tests stay in `iyakki-apps`; the 9 fake-port tests moved to `iyakki-tile-tales/src/fake_tests.rs` and pass in the crate. The nine story files left `migrated_tiles.list` (the list only shrinks).
  * Test fixes on the way: the PDF fake-port test waits for the picture thread; every test that touches the process-wide clipboard takes a new `iyakki_testkit::names::CLIPBOARD` lock; the chat paste-picture test runs with a home of its own under the ENV lock (it used to write `Pasted-picture-N.png` into the real `~/Uploads`, so the numbers grew with every run on a dev machine).
* Allow lines added: `iyakki-tile-tales` -> `iyakki-story-model`, `iyakki-story-kit`, `iyakki-voice-model`, `iyakki-chat-model`, `iyakki-scene`, `iyakki-piclog`, `iyakki-provider-kind`, `iyakki-chatview`, `iyakki-sysinfo-model`, `iyakki-web`, and dev-only `iyakki-sec`, `iyakki-filescope`, `iyakki-testkit`. OWNER: confirm. No core-api change.
* Remaining: none for the extraction. `quick_engines` (names `ProviderCfg`) stays in `iyakki_shared::story` and is called through the port.

## D-AP1-03 (alpha.11 AP1): `iyakki-tile-voice` exists; the voice desk, the assistant's state machine and the traits moved to `iyakki-voice-model`
* Question: how does the VOICE screen leave `iyakki-apps` (D-P3-01, D-Q4-01)?
* Default implemented (D-S2-01 option (a)):
  * `iyakki-voice-model` now also holds `voicedesk` (microphones, recorders, capture, pause detector, read voices, destinations, commands; `record_cmd_for_with` and `no_mic_text_with` take the two facts that used to come from the agent's capture-format table and the drivers' model list, and `iyakki_shared::voicedesk` keeps the three wrappers `record_cmd_for`, `no_mic_text`, `no_mic_text_t2` and `reword_storage_error` that read those), `recordings`, `voiceassist` (the state machine) and `voiceassist_ports` (`Cmd`, `Live`), and `facets` (the traits `SpeechKit`, `MineWorker`, `VoiceDeskFacet`, `PendingWaker`, `NoSink`, `NoSpeechKit` and the `Wake` enum; `iyakki-agent-api` and `iyakki_shared::pending` re-export them). New direct tests: 4 (record command, the no-microphone words, the kit defaults, the wake turns). `iyakki_shared::{voicedesk, recordings, voiceassist, voiceassist_ports, voicehelper, facets}` re-export the leaf, so no caller changed.
  * `iyakki-uikit::pathpick` (the Browse button and picker) moved from `iyakki-apps`, which re-exports it.
  * `iyakki-tile-voice` (dependencies: uikit, `iyakki-voice-model`, `iyakki-story-model`, `iyakki-speechtime`, `iyakki-filescope`, `log`): the seven screen files, `Flow`, `VoicePort` (the old trait plus `tile_recording`, `new_reader`, `speaks_directly`, `give_saved`, `stamp`, `no_mic_text`, `record_cmd_for`, `reword_storage_error`), `Fixer`, `Rig` (the recorders, the sink and the output check; the shell builds the person's rig in `apps/voice.rs::rig_for_user`, which names the browser session). `apps/voice.rs` is the `Deref` adapter (like LAB and TALES); `apps/voice_port.rs` keeps only `CxPort`. The 99 screen tests (and the recording, assistant, browser, stream and live test files) stay in `iyakki-apps` over the real services; the 9 fake-port tests moved to `iyakki-tile-voice/src/fake_tests.rs` and pass in the crate. The seven files left `migrated_tiles.list`.
  * The wake phrase stays experimental, off by default and labelled: nothing in the tile changed (the "EXPERIMENTAL, off by default" line and the opt-in tap are the same code); the tile only learns whether to tell the wake listener that it records through `tile_recording`.
* Allow lines added: `iyakki-tile-voice` -> `iyakki-voice-model`, `iyakki-story-model`, `iyakki-speechtime`, `iyakki-filescope`, and dev-only `iyakki-testkit`. OWNER: confirm. No core-api change; two rows of `compat-surfaces.txt` (`T_VERSION_MENU`, `T_VERSION_PICK`) name the story tile's new file (same values).
* Remaining: none for the extraction. Hardware proofs (a real microphone, the wake phrase on a machine) stay in Manual 'needed'.
## D-BOOT-ARM (decided by the model on the owner's delegation, 2026-11-24): `iyakki-boot` is the boot loader of the ARM boards

Owner: "there is an `iyakki-boot` folder that is our custom boot loader; that should handle Raspberry Pi and Kria KV260." The loader (`iyakki-boot/`, Rust, `boot-core` no_std plus `boot-uefi`) is built for `x86_64-unknown-uefi` only; its plan (`iyakki-boot/docs/PLAN.md` section 2 and step 6) names `aarch64` as "the same code". The first Pi 5 and KV260 images (alpha.10, lanes ARM1 and ARM2) do not use it: the Pi goes from the board firmware (`config.txt`) straight to the kernel and initramfs, the KV260 from its U-Boot through `extlinux.conf`; both continue in `iyakki-stage1`.

**Decision:** adopted. The two first images stay as the tested baseline and the fallback; new variants put `iyakki-boot` in front.
* KV260: `iyakki-boot` built for `aarch64-unknown-uefi` as `EFI/BOOT/BOOTAA64.EFI` on the card; the board's U-Boot (`zynqmp_kria.env`) tries extlinux, then `boot.scr`, then `efi/boot/bootaa64.efi`; the variant has no `extlinux.conf`, the baseline keeps it.
* Raspberry Pi 5: the firmware cannot start an EFI program; U-Boot (GPL-2.0+, downloaded by the person at their click with the licence shown first, never shipped) is started as `kernel=u-boot.bin` and starts the loader; the archived community edk2 for the Pi 5 is the alternative only if U-Boot fails.
* One configuration: the cards' boot files (the Pi's `config.txt`, `cmdline*.txt`, `iyakki-boot.txt`, `tryboot.txt` and the KV260's `extlinux.conf`) are generated from one board table, and the loader's `boot.cfg` (entries, boot-tries counter, one-shot choice, rollback) replaces the Pi's `iyakki-boot.txt` lean/full fallback, so there is one menu and one rollback for every board. The generator lives next to `boot-core`, not in `iyakki-usb`.
* Order: after the owner's first-boot reports of the two baseline images.
* Progress 2026-11-24 (lane BOOT1): the loader builds for aarch64 and passes its QEMU tests (also with the real KV260 kernel); `board-cfg` (the generator) reproduces the baseline files byte for byte; two loader cards were built next to the baselines (not switched, nothing shipped); U-Boot for the Pi 5 is pinned (Debian package, size and sha256) with the button still to write; rows `F-BOOTARM-01` to `06`, guides `docs/BOARD-RPI5-LOADER.md` and `docs/BOARD-KV260-LOADER.md`. Open: how the running system's `BootConfirm` reaches the loader through U-Boot (runtime variables), U-Boot's SD numbering on the Pi 5.

## Lane B1 (2026-11-24): the booted UI program (model decisions, the owner may overrule)

* **D-B1-01 added:** serve the dev-input calls (`take_input`, `set_secret_focus`, `set_approval_card`) and the dev-mode API over the UI socket, so `dev input` and Settings, Developer mode work in `iyakki.ui=process`. Without it the screen-driving scenarios cannot run against the program. Reason: it blocks the test suite on the split; scopes stay fixed and the calls are audited.
* **D-B1-02 added:** the supervisor forwards the program's stderr to the core log (bounded, rotated) instead of /dev/null, so the reason for an exit is kept next to the exit status.
* **D-B1-03 parked:** `IYAKKI_WITH_UI_PROGRAM=1` as the default of lean and release images (about 46 MB compressed in the ui layer). Park until process mode is the default and D-B1-01 is done; today the core layer still holds the whole shell, so the ui layer is a second copy.

## F-LIFE-14 (2026-11-24, model decisions, the owner may overrule)
* **D-LIFE14-01 added:** the row asks for route-lost pausers (BtRoute), not session-end hooks. The video tile's players are paused (picture and sound wait, press play); the external music player cannot be paused, so it is stopped with its queue and place kept. A lock still does not end playback (D-SESSION-END); the video tile stays a Sound kind there.

## Store tile crate, shell-only modules (2026-11-24, model decisions, the owner may overrule)
* **D-AP3-01 added:** the new crate `iyakki-tile-store` holds the engine trait (`StoreEngine`, step (a), merged) and, as steps (b) and (c) go, the Store screens. The Store files name shell-only modules (`agent_login`, `apps::cluster_host`, `samples`, `filepicker`, `touchpad`, `input`, `TESTING`). Decision: the file picker, touchpad and input helpers are moved to `iyakki-uikit` where they are pure (the Tales and Voice moves did the same with `pathpick`); `agent_login` and `cluster_host` stay in the shell and are reached through small ports of the tile (`StoreShellPort`: take and give the login request, the cluster switch request); `samples` (the skills text) is moved to `iyakki-store-model` because it is data of the store. `crate::TESTING` becomes a plain `cfg(test)` or a parameter.
* **D-AP3-02 added:** the agent-side paths (drivers, firmware, AV build, data science, clusters, GPU, MCP and release info) are reached through one small port trait per group in the tile crate with plain answers (mirror types where a type cannot move), the real impl in `apps/store_port.rs` over the facets; pure `iyakki_shared::store_*` paths are renamed to `iyakki_store_model::*` mechanically in step (c). The `pkg` build helpers move to `iyakki-store-model` only if they have no agent dependency; otherwise they stay behind the port.
* **Order:** `store_rating.rs`, then `store_drivers.rs`, then `store_setup*.rs`, then `store.rs`; one commit per file; the crate move (c) only after no Store file names an agent-side path.

## Evidence runs of batches F and G (2026-11-24, model decision, the owner may overrule)
* **D-EV-01 added:** the first two runs (lane EV1, 63 scenarios, same result both times: 47 PASS, 15 FAIL, 1 SKIP) ran on the old pilot image (program e3ed2d8c over an older base), so the 15 FAILs are features the scenarios name that the image does not have yet, not product faults, and the migrator refused every raise ("the scenario does not say `provided-by: program`"). Nothing is raised from those runs and no scenario is changed to satisfy the enforcer. The runs are repeated twice on the image the release gate builds from the release commit (base and program then match), after the gate, and the migrator is run on those results. The FAIL list stays as the check list for that run (a scenario that still fails on the fresh image is a finding).
* **D-AP3-03 added (2026-11-24, model):** the Store Setup tab's three open groups. (1) AI hardware profile: a port group `AiProfilePort` in the tile crate with a plain `Vis` mirror (Show, Recommended(text), NotRecommended(text)); the real impl in `apps/store_port.rs` over the agent's `ai_profile()`. (2) `HwView` and `AiMachine`: plain answers per question (the tab asks one at a time), no agent type in the tile. (3) the agent helpers `webengine_host_session`, `voicepack_delete_local_model` and `voicepack_review_lines`: one small helper group in the port with plain answers. Same pattern as the drivers chip (`DriversView`).

## Camera rows are proven by automation (2026-11-24, owner: "Camera can be tested by automation"; model decisions on how)
* **D-CAM-AUTO-01 added:** no human checklists for the F-CAM rows (those written by lane MC-C are removed). The network-camera rows (F-CAM-01, 03, 04, 05, 15, 17, 18, 19, 20, 21 and the camera tools of F-CAM-11) get QEMU scenarios against a fake camera server started by the session helper on the HOST (the lean guest has no Python; the guest reaches it at 10.0.2.2 through QEMU user networking, lane CAM1 built it in `dist/autotest/fakecam.py`) (an RTSP and a Motion-JPEG over HTTP source that draws a test pattern and a moving block on a schedule, so motion detection, the inbox notice with its picture, recording and the assistant's snapshot tool can all be checked from `dev state`, the files and the audit log). This needs the `net` boot profile of docs/development/AUTOTEST.md (fake services on loopback), which also unblocks F-ROUTER-13, F-RAG-14, F-APPSTORE-08 and F-LINAPP-03/04. Feature day starting 20:00 on 2026-11-24.
* **D-CAM-AUTO-02 added:** the local V4L2 rows (F-CAM-06, 07, 13) use the kernel's virtual test camera (`CONFIG_VIDEO_VIVID=m`, `CONFIG_V4L_TEST_DRIVERS=y`) loaded in a QEMU test profile, so the CAMERA tile and the in-house capture run on a real V4L2 node. The kernel fragment is NOT changed before the alpha.10 gate (a changed `linux.fragment` makes the image rebuild the kernel); it is changed after the gate, with the targeted kernel rebuild recipe, and the module goes into the test layer only. The MacBook's built-in camera (F-CAM-12, its driver node) and real USB cameras stay real-hardware evidence on the Mac mini and the MacBook.

## Sound is proven by recording and listening (2026-11-24, owner: "sound can be automated, by recording and listening"; model decisions on how)
* **D-SND-AUTO-01 added:** two layers, both automated. (1) QEMU: the guest's playback is recorded by the emulator itself (`-audiodev wav,id=snd0,path=RESULT/guest.wav` instead of `-audiodev none`, with the `intel-hda` and `hda-duplex` devices the sound tests already use), and a small in-house listener in `dist/autotest` (plain Python, no dependency: Goertzel for one frequency, RMS and peak in dBFS, left and right channel, silence and duration, clipping) judges it. New scenario verbs: `expect-tone HZ [tolerance] [channel]`, `expect-level LOW HIGH`, `expect-silence`, `expect-sound-within SECONDS`, `expect-speech MIN MAX` seconds (voice activity: energy envelope of a spoken piece lasts about as long as its text allows). The recording is kept as evidence next to the screenshots. Read-aloud and voice rows check the words against the screen's highlighted text and the duration, not by speech recognition. (2) Real machines (Mac mini 2018, Lenovo, MacBook): the machine plays a tone or a piece through its speakers or headphone jack and records it with its own microphone (or a line cable), through the device's own capture path; the recording is analysed on this PC with the same listener, at level `machine-<name>`. This raises sound rows to Test 100 on a machine without a person present; a person is only needed for what a recording cannot judge (a cable or headset that must be plugged in, how a voice feels).
* **D-SND-AUTO-02 added:** the microphone path in QEMU needs a virtual capture device; `CONFIG_SND_ALOOP` is not in the kernel fragment. Same rule as the camera: the fragment is not touched before the alpha.10 gate; after it, `snd-aloop` goes in as a module of the test layer and a playback helper feeds a WAV file into the loopback capture. Until then, the microphone rows are proven on the real machines with the speaker-to-microphone loop of (2).
* **Order:** feature day from 20:00 on 2026-11-24: the listener and the `wav` capture first (output rows: F-SND, F-BELL, F-RADIO, F-MEDIA, read-aloud of F-TALES and F-VOICE), then the machine layer, then `snd-aloop`.
* **D-AP3-04 added (2026-11-24, model):** step (c), the crate move of the Store screens into `iyakki-tile-store`, follows the Tales and Voice moves: option 1. `App`, `Flow` and the pure parts of `ScrollLog`, `blink` and the widgets the Store uses move to `iyakki-tile-api` and `iyakki-uikit` as they did for Tales; `filepicker::PathKind` and `scrolllist::ScrollList` move to `iyakki-uikit` (both are pure widgets); `chatpic::PicCache` stays behind a small picture port of the tile. The screens' tests that need the real `AppCx` stay in `iyakki-apps`. Done on the feature day from 20:00; nothing of step (c) is in alpha.10.
