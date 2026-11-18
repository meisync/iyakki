# SBC window tables: provenance, procedure, measurements (Q23)

Status 2026-11-09: the designed tables are in the tree and tested. The default build still uses the OLD tables (the ones heard on the
MacBookPro14,1) until the owner has compared them. Nothing here is committed or shipped yet.

## 1. What the tables are

SBC (the A2DP codec) splits the sound with a cosine-modulated filter bank of 4 or 8 bands. Its only large constants are the two
synthesis windows `D4` (40 values) and `D8` (80 values): a lowpass prototype filter of `10 M` taps (M = bands), symmetric, first tap
0, multiplied by a constant `K` (so that analysis then synthesis has gain 1) and with the sign flipping every `2 M` taps.
Analysis uses the same window divided by M. Code: `crates/iyakki-devctl/src/sbc.rs` (filter bank), `sbc_tables.rs` (tables),
`sbc_design.rs` (procedure), `sbc_tables_legacy.rs` (old tables, feature-gated).

## 2. Provenance of every constant

| Constant | Where | Provenance |
|---|---|---|
| Frame layout, sync 0x9C, header bit fields, sampling rates, block counts, CRC-8 (poly 0x1D) | `sbc.rs` | A2DP specification description, written from the format; checked by tests and by black-box interop |
| Loudness offset tables `OFFSET4/OFFSET8`, bit allocation procedure, scale-factor and quantiser formulas | `sbc.rs` | A2DP specification (recalled by the previous agent, not copied from any code); bit allocation must match exactly for a foreign frame to decode, and the black-box checks below decode at 47-65 dB, so they are right behaviourally |
| `D4_DESIGNED`, `D8_DESIGNED` | `sbc_tables.rs` | DESIGNED here (section 3). Not the specification's table values. Not derived from any other implementation |
| `D4_LEGACY`, `D8_LEGACY` | `sbc_tables_legacy.rs` | MEASURED from ffmpeg's decoder output (ffmpeg is LGPL/GPL). Own measurement of a third party's output: provenance not clean. Feature-gated, not for release |
| Design settings (Kaiser start beta 8, stop-band edge 1.1 pi/M, weight 1, grid 512, 300 iterations) | `sbc_design.rs` | Chosen by us by a sweep (section 4), judged only by the filter bank's own reconstruction error. The legacy tables were NOT used as a target or a tuning input |

I could not recall the specification's Appendix B window values with certainty, so none were typed in. The designed window is
therefore NOT bit-identical to the specification's and is not claimed to be.

## 3. The procedure (spec-derived vs designed)

Spec-derived (structure only): `10 M` taps, symmetric about tap `5 M`, tap 0 equal to 0, cosine modulation with phase
`(k + 1/2)(i + M/2) pi / M`, sign flip every `2 M` taps in the synthesis window, analysis window = synthesis window / M.
Textbook theory (pseudo-QMF filter banks, published in the early 1990s): a cosine-modulated bank reconstructs nearly perfectly if the
prototype's amplitude `A(w)` is small beyond one band (stop band) and `A(w)^2 + A(pi/M - w)^2 = 1` in the transition band.

Designed (`sbc_design::prototype`): the `5 M` free coefficients minimise, by Levenberg-Marquardt least squares with an analytic
Jacobian, (stop-band energy for `w >= 1.1 pi/M`) + (power-complementarity error on `[0, pi/M]`), starting from a Kaiser-windowed
sinc with cut-off `pi/(2M)`. Then `K` is chosen so that analysis followed by synthesis has gain exactly 1 (`designed_window`). Fully
deterministic (no random numbers; the test noise comes from a fixed xorshift). Regenerate with
`IYAKKI_SBC_OUT=file cargo test -p iyakki-devctl --lib print_designed_tables -- --ignored` and paste into `sbc_tables.rs`.

## 4. Measurements (unquantised filter bank, 4096 M samples of fixed noise)

| | 4 bands | 8 bands |
|---|---|---|
| delay (analysis + synthesis) | 37 = 9 M + 1 | 73 = 9 M + 1 |
| gain | 1.0000000 | 1.0000000 |
| reconstruction error (dB below signal) | -66.0 | -67.9 |
| stop band (>= 1.1 pi/M) | -61.9 dB | -65.9 dB |
| power complementarity error, max | 6.8e-4 | 4.1e-4 |
| pass band deviation (below 0.15 pi/M) | 1.8e-3 | 1.5e-3 |
| energy x 2M | 1.0003 | 1.0001 |
| old (legacy) tables, same test | -64.2 dB | -65.9 dB |

Sweep (6 stop-band edges x 4 weights): reconstruction error between -40 and -68 dB, best near edge 1.1; edge 0.8 and 1.5 are
clearly worse. Codec round trips with quantisation (existing tests, 50-65 dB with the legacy tables) pass with both windows.

I do not have the specification's own delay figure or its test sequences. The delay above is what the code measures.

Interoperability tolerance. Without the specification's conformance sequences, the tolerance is defined and measured as the
unquantised mismatch between an analysis window of one kind and a synthesis window of the other (the legacy window matches a real
decoder to about 65 dB, so it stands in for "behaves like the specification's"): legacy encoder to designed decoder, and designed
encoder to legacy decoder, both -36.9 dB (4 bands) and -39.9 dB (8 bands) (`designed_and_legacy_windows_interoperate_within_the_quantisation_noise`,
asserts below -30 dB). Black-box check against ffmpeg as a separate process (skipped when absent): ffmpeg decodes OUR stream with
the designed window at 39.2 to 40.1 dB (was 47.6 to 65.0 dB with the legacy window), and we decode ffmpeg's streams at 29.2 and
38.7 dB (was 29.5 and 44.5). So real devices will hear an error about 37-40 dB below the signal on top of the codec's own
quantisation noise (a typical bitpool gives roughly 25-45 dB). That is probably inaudible but it is worse than bit-exact tables, and only a listening
test can settle it.

## 5. What was removed

* The test `measure_window_tables_with_a_reference_decoder` (it ran ffmpeg to produce constants) is deleted.
* The ffmpeg tests `oracle_*` stay as BLACK-BOX interoperability checks: separate process, audio compared by SNR only, they skip when
  ffmpeg is absent, and no constant in the repository derives from them (labelled in `sbc_tests.rs`).
* Grep of `crates/` and `docs/` (target dirs excluded) for `libsbc`: no hit. For `bluez`: only "without BlueZ" statements in
  `att.rs`, `avdtp.rs`, `bthid.rs`, `btmgmt.rs`, `bt_pan.rs` and docs; no code or numbers. For `ffmpeg`: about 80 files, all uses of
  ffmpeg as an optional downloaded tool (AV add-on, player, lab, store, docs, licence notes), the SBC oracle tests, the legacy-table
  header and one md5-line fixture in `iyakki-archive/src/xz.rs` (a vendor checksum file format example, not a codec constant). No
  codec table or filter constant besides `sbc_tables_legacy.rs` was found to come from ffmpeg, libsbc or BlueZ.

## 6. Licence statement

The designed tables are produced by our own optimisation from textbook conditions; no GPL/LGPL code, table or output was used to
produce, tune or check them (the black-box checks only compare sound). The legacy tables are an own measurement of ffmpeg's decoder
output; the owner's rule is not to rely on that, so they must not ship. Release guard: with `IYAKKI_RELEASE_BUILD=1` in the
environment, the test `a_release_build_does_not_use_the_legacy_window` fails when the legacy window is selected.

## 7. Switching and the A/B test for the owner

* Cargo feature `legacy-sbc-window` of `iyakki-devctl`, ON by default now (that is what was heard on the MacBookPro14,1). Constant
  `sbc_tables::WINDOW_SOURCE` says which is in use.
* Old sound (default): build as usual. Designed sound: build the agent with `--no-default-features` for `iyakki-devctl`
  (for example set `default-features = false` on the `iyakki-devctl` dependency in the workspace `Cargo.toml`, or pass
  `--no-default-features` when testing the crate alone). The caller is expected to change that workspace line; this work did not touch it.
* Steps: (1) build and deliver one pack with the old window, play a fixed song from the phone for a minute, note anything odd (hiss,
  dull highs, clicks); (2) build and deliver a pack with the designed window, same phone, same song, same volume, switch back and
  forth; also try a 4-band stream (some phones) and a laptop; (3) if the designed one sounds the same, set
  `default = []` in `crates/iyakki-devctl/Cargo.toml`, delete `sbc_tables_legacy.rs`, the feature, and the legacy test, run with
  `IYAKKI_RELEASE_BUILD=1`; (4) if the designed one sounds worse, keep the legacy default for the alpha, and the next step is a better design
  (more taps are not allowed, so a different optimisation criterion) or the specification's exact table typed from the
  specification document by someone who holds it.
* Still needing a real device: phone playing music through the sink with each window; a phone's encoder at 8 bands and at 4 bands;
  the specification's conformance sequences, which we do not have.
