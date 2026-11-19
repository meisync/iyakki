# Compatibility fixtures

One folder per release holds what that release wrote or said on the wire, recorded once, never edited afterwards. Every later release must
read all of it (backward) and must keep unknown parts (forward). The rules are in
[docs/development/CORE-COMPATIBILITY-DESIGN.md](../../docs/development/CORE-COMPATIBILITY-DESIGN.md); the list of versioned surfaces is
[docs/development/compat-surfaces.txt](../../docs/development/compat-surfaces.txt).

## What is in `alpha2/`

| Path | What | Tested by |
|---|---|---|
| `client-session.jsonl` | a transcript of the client session protocol (`hello`, `enrol`, `auth`, `sign_in`, `call`, ...), one `{"dir":"c"\|"s","msg":{..}}` per line | `iyakki-core-api/tests/compat_alpha2.rs` |
| `contract-hello.json` | the typed contract `hello` of a machine | same |
| `capability-catalogue.txt` | every capability name with parameter kind, built-in default, hard-ask flag | same |
| `app.json` | an app manifest with a `capabilities` list | same |
| `extension-snapshot.hex`, `extension-snapshot-v9.hex` | the browser extension snapshot bytes, and a snapshot of a newer layout | `iyakki-webview/tests/compat_alpha2.rs` |
| `helper-frames.txt` | frames of the helper socket (`Hello`, `Navigate`, `Ready`, `Title`) | same |
| `data/` | a tree standing in for `/var/lib/iyakki`: every JSON store with an `iyakki_schema::Spec`, `store/approvals.json`, `access/log.jsonl`, `views-amy.json`, `approval-pins-amy.json` | `iyakki-agent/src/compat_tests.rs` and `compat_tests_schema.rs` (findings F-2 to F-4: migration, damaged, newer; the pins also in `iyakki-shell`) |

Everything is synthetic: fixed ids, times and keys, `example.com` style names, no real token, password, key or personal file.

## What is in `alpha3/` besides the media transcripts

| Path | What | Tested by |
|---|---|---|
| `connect-agent-key.txt` | the key file of a connect device key that an ssh-agent holds (a public key, no secret) | `iyakki-connect` devkey `agent_tests` |
| `bios-save-v1.hex` | a BIOS boot save file (sectors saved before a legacy-BIOS install) | `iyakki-usb` `biosshared` tests |
| `checkup-result.json` | a signed CHECK-UP result, made with a throw-away key | `iyakki-agent` `checkup_result` tests |

| `binary-delta-v1.hex` | a binary delta (`IYBDELT1`) of a made-up old and new file | `iyakki-bindelta` tests (`the_delta_alpha3_wrote_...`) |
| `volume-header-v1.hex` | an encrypted-volume header (`IYAKVOL1`, one slot, made-up password, cheap key cost) | `iyakki-volcrypt` `header_tests` |
| `recording-v1.hex` | a camera recording segment (`IYAKREC1`), two frames under a made-up key | `iyakki-camdetect` `camrec_format_tests` |

| `backup-v1-plain.hex`, `backup-v1-sealed.hex` | a backup file (`IYAKKIBK`, format 1) of five made-up files, plain and sealed under a throw-away passphrase | `iyakki-agent` `compat_formats_tests` (`the_plain_backup_...`, `the_sealed_backup_...`, `a_newer_format_a_flipped_byte_...`) |
| `snapshot-v1/<id>/` | a cloud snapshot folder (`manifest.json`, `manifest.sha256`, `data.tar.zst`) of three made-up files | same (`the_snapshot_alpha3_wrote_...`, `a_changed_snapshot_archive_...`) |

| `wake-weights-v1.hex` | a wake-phrase weights file (`IYWK`, version 1): header, notes and a made-up network | `iyakki-wake` `tests/compat_alpha3.rs` |
| `knowledge-base-v1.hex` | one knowledge-base namespace file (`*.afkb`) of two made-up documents, built-in hashing embedder | `iyakki-memx` `tests/compat_alpha3.rs` |

| `contract-hello-0.2.json` | the typed contract `hello` of contract 0.2 (the base64 chunk encoding), one machine | `iyakki-core-api/tests/compat_step4.rs` |

The recorders are the ignored tests `record_alpha3_*` (`IYAKKI_COMPAT_RECORD=1`; the wake and knowledge-base ones print the file between BEGIN-FIXTURE and END-FIXTURE lines: `cargo test -p iyakki-wake --test compat_alpha3 -- --ignored --nocapture`). `users.json` is covered by `tests/fixtures/upgrade/`.

## What the tests check

* **Backward:** the current code reads every file and every message and finds what was written; reading changes nothing and makes no backup.
* **Forward, unknown fields:** a field a newer release added is still in the file after the store wrote it; a message with an extra field is
  processed without it.
* **Forward, newer schema:** a file with a higher `schema` is read, a write is refused with the sentence that says to update IYAKKI, the bytes
  on disk do not change, and the file is not treated as damaged.
* **Forward, unknown kinds:** a hook kind or message type nobody here knows is skipped and reported, never a crash.
* **Refusals name the side:** a version mismatch says whether the client or the instance must be updated.

## Adding a release folder (when a release is cut, or a surface changes)

1. Cut the folder from the release's own code, not by editing files: check out the release (or work on the release branch), then run the
   recorders, which write into the folder named in their source:
   * `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-core-api --test compat_alpha2 record -- --ignored`
   * `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-webview --test compat_alpha2 -- --ignored`
   * `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-agent --lib record_alpha2 -- --ignored`
2. Copy the three test files and the recorders for the new release (`alpha3` and so on): the old folder and its tests stay as they are, so
   the new code keeps reading the old files. A fixture is only ever added, never changed to make a test pass.
3. Add the new surface or bump to `docs/development/compat-surfaces.txt`.
4. Surfaces that sit in a crate other than the ones above add their own recorder and a test in that crate (a vault, a ledger, a recording
   header, a wizard spec, a bundle manifest are owed; see the charter, section 9).
5. Run `./run.sh compat-check`.

## Test hygiene

`crates/iyakki-agent/tests/test_hygiene.rs`: a static check that no test reads the real `HOME` (mark a read-only use with `// home-ok: why`), and, with
`IYAKKI_HOME_HYGIENE=1` (`dist/release/home-hygiene.sh`, also run by `./run.sh compat-check` when the variable is set), a run of the unit tests of
`iyakki-agent` and `iyakki-flow` that fails if the names in `$HOME`, `$HOME/Flows` and the other well-known folders change. `./run.sh test` sets
`IYAKKI_NO_HOST_HOME=1`, so the Flows folder of an account never falls back to the real `$HOME`.

## `alpha3/` (media half of the client protocol, EXPERIMENTAL)

| Path | What | Tested by |
|---|---|---|
| `client-media.jsonl` | sound, clipboard, camera and device-offer messages of the client session, one `{"dir":..,"msg":..}` per line | `iyakki-core-api/tests/compat_alpha3_media.rs` |
| `capability-catalogue-added.txt` | the three capabilities this release added to the catalogue (`clipboard.client`, `client.camera.offer`, `client.microphone.offer`), all hard asks | same |
| `binary-frames.txt` | the binary payloads of streams 2 to 5 as hex | same |

Recorded with `IYAKKI_COMPAT_RECORD=1 cargo test -p iyakki-core-api --test compat_alpha3_media record -- --ignored`. The alpha2 folder stays as it is.

`alpha3/vault-v1.json` is a vault file (one key slot, two made-up secrets, the throw-away password in `compat_formats_tests.rs`), recorded by `record_alpha3_vault`; the current code must open it.

## What is in `alpha5/`

| Path | What | Tested by |
|---|---|---|
| `display-list-v2.txt` | a display list of version 2: a `blur` and a `textx` run (family, letter spacing, tabular figures) next to plain `text` | `iyakki-ui-sdk` `type_tests::the_display_list_alpha5_wrote_is_read_and_written_back_byte_for_byte` |

## What is in `alpha7/`

| Path | What | Tested by |
|---|---|---|
| `contract-hello-0.3.json` | the typed contract `hello` of contract 0.3 (the `appevents` domain, F-SDK-202), one machine | `iyakki-core-api/tests/compat_step4.rs` (`the_contract_0_3_hello_of_alpha7_...`; recorded by `record_alpha7_contract_hello_0_3`) |
