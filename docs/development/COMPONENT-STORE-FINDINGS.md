# Component store findings: what blocks a `system-component` kind of 400-500 MiB

Status: read from the code 2026-11-07; the first slice was built the same day (section "What changed" at the end). The table below describes the code BEFORE that change. Companion to [`COMPONENT-UPDATES-PLAN.md`](COMPONENT-UPDATES-PLAN.md); the deployment is in `dist/store-k3s/`.
Not measured: the memory use of a 200 MiB publish. The server refuses anything above 10 MiB (below), so there is nothing to measure without the code change first.

## What stops it today (in the order a publish meets it)

| # | Where | Rule today | Effect on a 400-500 MiB component |
|---|---|---|---|
| 1 | `store/server_http.rs` `run_cli` | `--max-body-mib` is `clamp(1, 10)`; default in `Config::new` is 4 MiB | Hard ceiling of 10 MiB whatever the flag says. A larger `Content-Length` is answered 413 before the body is read (`len > max_body + 1024`). |
| 2 | `store/server_http.rs` `read_request` | The whole body is read into one `Vec<u8>` (`body.extend_from_slice`), 10 s read timeout per read, one thread per connection (64 max); `Transfer-Encoding: chunked` is refused | A 500 MiB upload needs 500 MiB in memory once here. |
| 3 | `store/server.rs` `publish` / `package_bytes` | `r.body.clone()` for a raw body (a second copy); multipart is searched in memory | 2 copies (1 GiB) before parsing. |
| 4 | `store/server.rs` `accept` | `std::str::from_utf8(bytes)`: a package must be **text**; the payload is base64 (about 4/3 larger) | 500 MiB of artifact becomes about 667 MiB of text. `pkg::parse` also does `replace("\r\n", "\n")`, which allocates a third and fourth copy of the whole text. |
| 5 | `pkg.rs` `parse` and `max_bytes` | `parse` refuses text over `MAX_EXTENSION_PACKAGE_BYTES` = 12 MiB for every kind; per kind: default 64 KiB, tool about 2 x wasm cap, tile 3 MiB, extension 12 MiB | Nothing above 12 MiB is parseable at all, even if the kind cap were raised. |
| 6 | `store/server.rs` `DEFAULT_KINDS`, `Config.kinds` (set by `Config::new`, no flag exposes it) | Allow-list: skill, tool, mcp, agent, template, theme, extension, tile | `system-component` gives 422 "this store does not take system-component packages". `store/vet.rs` has per-kind branches (`skill`, permission checks for `skill/template/theme/agent/extension`) but no allow-list of its own; an unknown kind falls into the generic path. |
| 7 | `store/vet.rs` | `MAX_FILES` 400, `MAX_FILE` 512 KiB per file in a project dir; a secrets scan over the payload | The scan reads the whole payload; a binary blob may produce false positives. |
| 8 | `store/server.rs` `GET /packages/<sha>.iyp` | `std::fs::read` of the whole file into memory, then `write_resp` (one `write_all`); write timeout 30 s | A download costs the full size in memory per concurrent client; a slow device on Wi-Fi may hit the 30 s write timeout on 500 MB. No `Range`, so no resume. |
| 9 | `regenerate` | `state.json`, `catalog.json`, `yanked.json` are rewritten completely on every change | Fine: they hold metadata only (one entry is about 1 KB), so the catalog size is not the problem; 3 versions x 5 components stays under 100 KB. |
| 10 | Rate limit | 20 writes a minute per address | Not a problem. |

Packages are stored as `packages/<sha256>.iyp` and never rewritten, which is what the plan wants (immutable, cacheable). The catalog entry already has `size` and `sha256`.

## Minimal code changes (not made)

1. **Keep the 4-10 MiB path for apps untouched** and add a separate route for components so the memory and size rules of apps do not loosen: `POST /v1/publish-component` (administrator or a publisher with a `components` scope) that streams the request body to a temporary file in `<data>/incoming/` while hashing, with its own `--max-component-mib` (default 600) and a longer read timeout (progress-based, not 10 s per read). Memory use stays at one 64 KiB buffer.
2. **A binary package form for components:** a signed small text header (the existing header lines plus `type: system-component` and the component fields: component, version, requires, delta references), then the raw bytes, with `sha256` over the raw bytes. The signature still covers the header including the hash, so integrity is unchanged. This avoids base64 (33% less disk and traffic) and the `from_utf8` / `replace` copies. `pkg::parse` gets a header-only reader for this form; `MAX_*_PACKAGE_BYTES` stay for the text kinds.
3. **Allow-list:** add `system-component` to `DEFAULT_KINDS`, but accept it only on the new route (the old route keeps refusing it, so an app publisher cannot publish a component), and only from the administrator token or a publisher marked as a component publisher in `publishers.json`.
4. **Vet rules for the kind** in `store/vet.rs`: header fields validated (names, version, `requires` ranges parsed), no payload scan beyond the hash (the payload is a build artifact, not text), size cap per component (kernel 64 MiB, base image 600 MiB, core 150 MiB as examples), delta entries must reference an existing full artifact by sha256.
5. **Download:** stream `GET /packages/<sha>` from the file in chunks (open, `io::copy`) and support `Range` / `If-Range` so a device on weak Wi-Fi can resume; raise the write timeout to progress-based.
6. **Deployment:** PVC is already 50 GiB; the pod memory limit of 1 GiB is enough after change 1 and 5 (buffers only). Without them a 500 MiB publish needs at least 2.5 GiB of memory (the copies in rows 2-4), which is more than the namespace quota (2 GiB limits).
7. **Retention:** a `DELETE` today only yanks (the file stays on disk); the plan's cleanup needs a real removal of files that no live catalog entry or delta refers to, plus a size report.
8. Tests: a 20 MiB streamed publish with a memory assertion, a refusal test for the old route, a tampered header test.

Nothing in the trust model has to change: devices verify the signature and the hash themselves.

## What changed (first slice, 2026-11-07)

Built as proposed in "Minimal code changes", with these differences found while building it:

* **No `system-component` package kind and no change to `pkg.rs` or `vet.rs`.** Components never go through the package path, so the 10/12 MiB caps, the UTF-8 requirement and the `replace("\r\n")` copies (rows 3-5) do not apply and were not touched. `DEFAULT_KINDS` is unchanged and still refuses `system-component` as a package. The artifact is a raw blob named by its SHA-256 (`blobs/<sha256>`), and the signed `components.json` carries `{component, version, sha256, size, requires, channel, built_from, install_path, delta[]}`. Integrity: the device checks the list's signature against the pinned key and the blob against the list's hash; the blob needs no header or signature of its own.
* **Rows 1-2 (10 MiB, whole body in memory):** the socket code (`server_http.rs`) now recognises `/v1/blobs/` before the body is read: it hands the connection's reader to the handler, which reads through a 64 KiB buffer (asserted in the test by a reader that records the largest read it was asked for: 50 MiB uploaded, largest read 64 KiB). The 4-10 MiB limit and the 413 for other routes stay (tested over the socket with a 20 MiB `Content-Length` to `/v1/publish`).
* **Row 8 (download reads the file whole):** the blob route streams from the file with `Range`/`If-Range` and `HEAD`. `/packages/<sha>.iyp` still uses `std::fs::read` (apps are small).
* **Upload resume was added** (not in the proposal): `Content-Range` pieces plus `GET /v1/blobs/<sha>/part`, because a 500 MiB upload over Wi-Fi will be cut at some point. The publish command sends 32 MiB pieces, so the client holds one piece, and the hash of a multi-piece upload is checked by reading the part file once at the end.
* **Timeouts:** the existing 10 s per-read and 30 s per-write socket timeouts are kept; they are per read/write, so they already behave as stall limits for a stream. Not measured: a real 400 MB transfer over Wi-Fi.
* **Memory:** server side bounded buffers only (tests above). Device side the download uses a 64 KiB buffer, but **applying a delta reads the installed file and the delta whole and builds the result in memory** (`bindelta::apply_delta`, limit `MAX_FILE` 512 MiB), so a 400 MiB base-image delta peaks at roughly 3x the file (about 1.2 GB). Not measured here. If that is too much for the smaller devices, the fallback is the whole file (already the behaviour when a delta cannot be applied) or a streaming `apply_delta`.
* **Retention (row 7):** real removal of files exists now (`POST /v1/components/retention`), report-first. `DELETE /v1/packages/...` for apps still only yanks.
* **Deployment (row 6):** nothing in `dist/store-k3s/` changed. To run components there, start `iyakki store-server` with `--max-component-mib` (default 1024), optionally `--component-caps kernel=128,base=900` and `--disk-margin-mib`; the pod memory limit of 1 GiB is enough for the blob routes (buffers only). The PVC must hold the retention policy: newest 3 full artifacts per component and channel plus deltas, for example base 3 x 450 MB + core 3 x 80 MB + kernel 3 x 15 MB per channel before deltas.
* **Not yet decided:** which key signs the list in production. Today the catalog key signs it (like `system.json`), as the plan says only the update-signing key must stay on the host; a stricter variant would sign `components.json` on the host and have the store only serve it.
