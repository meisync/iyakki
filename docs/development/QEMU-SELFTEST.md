# QEMU boot test and the self-test

`dist/buildroot/qemu-test.sh [build-output-dir]` boots an image in QEMU with the kernel argument `iyakki.selftest=1` and checks the lines
the self-test prints on the serial console (`selftest()` in `crates/iyakki-core/src/main.rs`). It prints every `IYAKKI-SELFTEST` and
`IYAKKI-HWREPORT` line, then fails with `FAIL: missing '<line>'` for the first line it does not find, and ends with `PASS`.

## What the self-test checks

| Lines | Proves |
| --- | --- |
| `IYAKKI-HWREPORT ...` | the hardware report reads the (virtual) machine |
| `tool arecord|aplay|ssh|scp|dropbearkey ok`, `ssh Dropbear v...` | the programs the voice and CLI tiles start are in the image |
| `iface ...`, `addr eth2 ...` | the network adapters were found and DHCP gave an address |
| `wasm guest-ok` | the sandbox runs a guest |
| `path <name> ok` | **the smallest call of each runtime path works on this image** (below) |

## N1: the smallest call per path

Code that passes its tests with a fake runner can still fail on the real image when a program or the CA bundle is missing (`curl` was the
case: "curl is not installed on this system" in the Git wizard). `crates/iyakki-agent/src/pathcheck.rs` runs, in the booted image and
through the production code (no fake), one small call per path, against a **loopback listener the self-test starts itself** (127.0.0.1,
never the internet):

| Line | Path | The call |
| --- | --- | --- |
| `path ca-bundle ok` | every https call | the system's CA bundle loads with at least one certificate (`gitnet::ca_bundle_certs`) |
| `path programs ok` | all | `sh ip git ssh aplay arecord amixer` are on `PATH` (the `image` rows of `dist/buildroot/image-programs.txt`) |
| `path platform-connect ok` | Platform services connect | `platform::connect::NetHttp` sends `GET /api/v1/version` with a token |
| `path registry-probe ok` | "is the registry answering" | `netprobe::get_ok` and `get_body` on `/v2/` |
| `path registry-setup ok` | registry setup and push | `ocipush::TcpHttp` and the any-certificate reachability call on `/v2/` |
| `path cluster-tools ok` | cluster tools | the `wsexec::System` runner: `kubectl version --client` when kubectl is there; when it is not (it is downloaded with k3s), the missing program is reported as missing and explained in words |
| `path git-client ok` | the Git client | `gitnet::NetGit` with a token, and `git --version` through the runner |
| `path doctor-probes ok` | DOCTOR | a connection probe, the interface list (127.0.0.1 must be listed), the uptime and the hardware report |
| `path mp3-decoder ok` | the browser's MP3 plugin | web images: `gst-launch-1.0` decodes a 5 KB MP3 through `mpegaudioparse ! iyakkimp3dec` (`libgstiyakkimp3.so`, `dist/webview/wpe-helper/mp3dec.c`, minimp3, CC0); images without the web engine say so and pass |
| `path web-recorder ok` | the browser's recorder and sound output | web images: `gst-inspect-1.0` finds `encodebin`, `uritranscodebin` and `iyakkiwebaudiosink`; without the first two every recorder page fails with "The MediaRecorder is unsupported on this platform" |

A path that fails prints `IYAKKI-SELFTEST path <name> FAILED <why>` and `qemu-test.sh` fails on it with the log path. The loopback
interface being down also shows up here (the listener cannot start): that is an image fault, not a test fault.

Unit tests (`pathcheck_tests.rs`) run the same checks against the same listener on the development computer, and check that
`qemu-test.sh` requires every name in `pathcheck::PATHS`. When you add a path, add it to `PATHS`, to `check`, and to the list in the script.

## Status

Written and unit-tested 2026-10-28. The existing images were built before the step existed, so **the image must be repacked** (`repack.sh`)
before `qemu-test.sh` can show the new lines; the first QEMU run of these checks has not happened. `qemu-test-aarch64.sh` does not require
them yet.
