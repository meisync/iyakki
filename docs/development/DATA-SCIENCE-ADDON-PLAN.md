# Data science toolkit (scikit-learn, NumPy, SciPy, OpenCV): licence audit and plan

Status: **plan, nothing built.** Written 2026-10-25. Owner decision (same day): the toolkit is a **click-to-download Quick install item (Store -> Setup), never shipped in an image** (option (a) below). This document is the audit that backs that decision and the design for the next step.

Everything in section 1 was checked on 2026-10-25 against PyPI and the wheels themselves (downloaded into a scratch folder outside the repository, opened with `python3 -I`, METADATA and bundled licence files read, the libraries listed with `readelf`). What could not be checked is in section 9.

## 0. Summary

* **Ship in an image: nothing.** The Python-level code of NumPy, SciPy, scikit-learn, joblib, threadpoolctl is permissive (BSD-3-Clause and similar), but every binary wheel carries third-party libraries inside (`*.libs/`): LGPL-2.1+ `libquadmath`, GCC-runtime-exception `libgfortran`/`libgomp`, and in OpenCV an LGPL-2.1 FFmpeg, **Intel's proprietary IPP** (x86_64 wheels), and OpenSSL 1.1.1k (end of life). That is outside "permissive only", and the wheels need glibc.
* **Offer as a person-clicked download: yes.** Same rule as ffmpeg, Claude Code and the firmware packs (`LICENSING.md` items 5, 10): fetched from the vendor at the click, hash-checked, never in an image, never in the repository; the licence notices are shown at the click.
* **musl is a blocker for the useful part.** At the current versions PyPI has `musllinux` wheels for NumPy, SciPy, pandas, Pillow, but **none for scikit-learn and none for OpenCV** (checked in the file lists). So the toolkit runs in a **glibc place: the Linux apps (Debian) sandbox**, with a pinned python-build-standalone Python and wheels unpacked by IYAKKI's own code. Verified working on a glibc PC (section 3.3).
* **In-house first stays true.** Data generation, simple statistics and simple models do not need Python (section 2, option d); build the small Rust pieces anyway, and use the toolkit for what they cannot cover.

## 1. Licence audit (verified)

Versions are the newest on PyPI on 2026-10-25. "Declared" is the wheel's METADATA (`License-Expression`, `License`, classifiers).

### 1.1 Packages

| Package | Version | Declared licence | Bundled in the wheel (found in `*.libs`, `LICENSE*` files) | Copyleft or restricted item | Obligations | Verdict |
|---|---|---|---|---|---|---|
| Python (python-build-standalone, CPython 3.12.15, tag 20261021) | 3.12.15 | PSF-2.0 (permissive, GPL-compatible) | the build's own libraries (OpenSSL, SQLite, libedit, Tcl/Tk, ...) | none checked (see section 9) | PSF notice | download only |
| numpy | 2.5.4 | BSD-3-Clause AND 0BSD AND MIT AND Zlib AND CC0-1.0 | OpenBLAS (scipy-openblas) BSD-3-Clause, LAPACK BSD-3-Clause-Open-MPI, **libgfortran GPL-3.0-or-later WITH GCC-exception-3.1**, **libquadmath LGPL-2.1-or-later** | libquadmath (LGPL); libgfortran has the runtime exception | keep the notices (`LICENSE.txt` lists them); LGPL: the library stays a replaceable `.so` (it is), source offer if redistributed | download only |
| scipy | 1.18.1 | BSD-3-Clause (METADATA text, classifier BSD) | same OpenBLAS/LAPACK/libgfortran/libquadmath (two libgfortran copies), Qhull (own licence, permissive), DOP853 (BSD), uarray, duccfft | libquadmath (LGPL) | as numpy | download only |
| scikit-learn | 1.9.1 | BSD-3-Clause | **libgomp (GPL-3.0-or-later WITH GCC-exception-3.1)**, vendored array_api_compat/extra (MIT/BSD) | libgomp has the runtime exception (allows non-GPL programs) | notice | download only |
| joblib | 1.6.0 | BSD-3-Clause | needs `cloudpickle` (BSD-3-Clause, 3.1.2) | none | notice | download only |
| threadpoolctl | 3.7.0 | BSD-3-Clause | none | none | notice | download only |
| narwhals | 2.27.1 | MIT | none (new hard dependency of scikit-learn 1.9) | none | notice | download only |
| opencv-python-headless | **4.14.0.94** (5.0.0.93 audited too, same structure) | METADATA "Apache 2.0"; the packaging itself MIT (`cv2/LICENSE.txt`) | OpenCV (Apache-2.0), **FFmpeg libavcodec/libavformat/libavutil/libswscale/libswresample (LGPL-2.1; its configure line shows `--enable-shared --enable-openssl --enable-libvpx` (4.14 also `--enable-libdav1d`), no `--enable-gpl`); this FFmpeg is what reads video files and network streams for `cv2.VideoCapture` and `VideoWriter`**, libaom, libvpx, libdav1d, libavif (BSD), libpng, libdrm (MIT), **OpenSSL 1.1.1k** (OpenSSL/SSLeay licence, end of life since 2023), OpenBLAS 0.3.15 + libgfortran + libquadmath, libjpeg-turbo, libspng, OpenJPEG, libtiff, protobuf, flatbuffers, QUIRC, and **Intel IPP ICV statically linked on x86_64 (Intel Simplified Software License, binary only, no reverse engineering, no modification)** | **FFmpeg (LGPL-2.1)**, **IPP (proprietary-style)**, OpenSSL 1.1.1k (security, not licence) | LGPL: notice, licence text, replaceable library, source offer; IPP: reproduce Intel's notice, no other rights | **not shippable**; download only; the x86_64 build is the one with IPP (the aarch64 wheel should be checked separately, section 9) |
| opencv-python (not headless) | 5.0.0.93 | same | adds Qt 5 (LGPL-3.0) and X libraries | Qt LGPL-3.0 | as above | **do not use**: needs a display stack we do not have; headless covers every recipe here |
| pillow (optional) | 13.0.0 | MIT-CMU (HPND) | freetype (FTL or GPL-2.0: take FTL), harfbuzz (MIT), libjpeg-turbo (IJG/BSD), lcms2 (MIT), libtiff, libwebp, openjp2, libavif, libpng, brotli, xz (public domain), zstd (BSD), libxcb (MIT) | none in the binaries (the GPL/LGPL text in its `LICENSE` is xz's build system, not in the binary) | notice file | optional download; permissive; musllinux wheel exists |
| pandas (optional) | 3.1.0 | BSD-3-Clause | vendored klib, ultrajson, fast_float, musl-derived code, xsimd (all permissive); needs numpy, python-dateutil (dual BSD/Apache), tzdata (Apache-2.0), six (MIT) | none | notices (`LICENSES/`) | optional; not in v1 (Data tab already has an engine) |
| matplotlib (optional) | 3.11.2 | PSF-style matplotlib licence | DejaVu fonts (Bitstream Vera), STIX fonts (OFL), kiwisolver, contourpy, cycler (BSD), fonttools (MIT), pyparsing (MIT) | none | notices | optional; not in v1 (DASH draws charts) |

The wheels' own metadata mixes styles (SciPy and OpenCV have only the old `License:` text), so for those two the verdict rests on the bundled `LICENSE.txt` and `LICENSE-3RD-PARTY.txt`, which we read.

### 1.2 What is fine where

* **Fine to ship in an image:** nothing from this list. (Pure-Python parts like joblib, threadpoolctl, narwhals, cloudpickle are permissive and tiny, but are useless without NumPy.)
* **Fine only as a person-clicked download into the data folder:** all of it. The LGPL and runtime-exception libraries are then the vendor's files, unmodified, replaceable, and IYAKKI does not link or redistribute them. This is the same legal shape as the ffmpeg add-on and Debian's packages in the Linux apps sandbox.
* **OpenCV is the delicate one:** IPP's licence allows redistribution only unmodified with Intel's notice, and the bundled OpenSSL 1.1.1k is no longer maintained. For a download this is acceptable if we (a) show the notice at the click, (b) run it with no network (below), (c) say in the item text what is inside. A cleaner variant, if the owner wants it: build OpenCV ourselves without IPP and FFmpeg (a larger project, section 7).
* **Patents:** FFmpeg inside OpenCV decodes H.264/AAC and others; as with the ffmpeg add-on, using patent-licensed formats is the person's choice. Our recipes do not need video; the item text says so.

### 1.3 Python itself and musl

* CPython: PSF-2.0 (permissive). IYAKKI already lists Python from python-build-standalone as a download (`LICENSING.md` item 5, catalogue entry `python` in `clis.rs`).
* musl: python-build-standalone's default musl builds have been dynamically linked since 20250311 (its quirks page), so a musl Python can load extension modules, **but** the wheels are the limit. From the PyPI file lists on 2026-10-25:

| Package | musllinux cp312 wheel (x86_64 / aarch64) | manylinux (glibc >= 2.27/2.28) |
|---|---|---|
| numpy 2.5.4 | yes / yes | yes |
| scipy 1.18.1 | yes / yes | yes |
| pandas 3.1.0 | yes / yes | yes |
| pillow 13.0.0 | yes / yes | yes |
| matplotlib 3.11.2 | yes / no | yes |
| scikit-learn 1.9.1 | **no / no** | yes |
| opencv-python-headless 4.14.0.94 | **no / no** | yes (`cp37-abi3`, x86_64 and aarch64) |
| joblib, threadpoolctl, narwhals, cloudpickle | pure Python (`py3-none-any`) | n/a |

  An sdist build on the device (compilers, Fortran, BLAS) is out of the question. So the musl base image cannot run scikit-learn or OpenCV; a **glibc sandbox** can. The Linux apps Debian 12/13 sandbox supplies glibc (Debian 13 has 2.41; the wheels need 2.27/2.28) and the C++ runtime (`libstdc++.so.6`, `libgcc_s`, `libz` are what `cv2.abi3.so` needs; checked with `readelf -d`, presence in `debian:*-slim` to be confirmed by the first real run).

## 2. Options

| | (a) Quick install item, runs in the Linux apps sandbox | (b) use the Linux apps sandbox by hand (`apt`/`pip` inside) | (c) k3s job with a pinned python image | (d) in-house Rust only |
|---|---|---|---|---|
| What | **Recommended.** Download Python + pinned wheels at the click into the data folder; run in a glibc sandbox with no network | the person or agent runs `pip install` in a Debian sandbox | `sandbox_run` style pod, image pulled from a registry | extend `iyakki-data`/new crate |
| Disk | ~64 MB Python + ~62 MB core wheels (+ ~62 MB OpenCV) download; about 100 MB + 200 MB (+ 150 MB) unpacked after pruning tests, plus ~120 MB Debian base if no sandbox exists | same plus pip cache | image 150 MB to 1 GB, pulled by the cluster | a few 100 KB of code |
| Supply chain | **best**: exact URLs and SHA-256 in one table, wheels only, our own unpacker, nothing executed at install | pip resolves and may run sdist builds and install scripts | image digest can be pinned; contents opaque | none |
| Offline | works after install; install needs network once | needs network for every new package | needs the cluster and the image cached | always works |
| CPU on a laptop | runs on the host kernel, `nice`, thread cap (BLAS and OpenMP default to all cores: we set 2) | same | k3s overhead (tens of seconds, RAM) | lowest |
| Maintenance | one pin table to bump twice a year | high (open-ended) | medium | ours forever |
| Licence risk | download-only shape (section 1.2) | same, less controlled | image carries GPL/LGPL (already noted in item 5) | none |

(b) and (c) already exist for developers (`linux` command, `sandbox_run`); the toolkit is (a) done with a controlled supply chain. k3s is not needed.

**(d) In-house, what exists and what it cannot cover.** `iyakki-data` has a small engine (CSV paging, sort, filter, derive with a safe expression language, group-by with aggregates, limit, stats) and an optional Polars engine (join). It has **no random generator, no models, no matrix algebra**. A Rust `data_generate` (seeded PRNG, uniform/normal/categorical/date/id columns, missing values, a time series, simple noisy linear and logistic relationships) is a ~500-line job and would cover most "make me a test set" requests with no Python at all. Beyond it: k-means, linear/logistic regression and decision trees are a few hundred lines each and plausible later. **Cannot cover:** the breadth of scikit-learn (random forests with tuning, pipelines, metrics zoo, clustering variants, `make_classification` semantics), SciPy statistics and optimisation, OpenCV image processing and detection. That is what the toolkit is for.

**Out of scope:** GPU libraries (CUDA, cuDNN, cuML, OpenCV-CUDA, PyTorch, TensorFlow; see `GPU-K3S.md`), deep-learning frameworks, Jupyter, Dask, compiling from source, conda, the non-headless OpenCV, `opencv-contrib` (non-free algorithm modules; separate licence review), Windows/macOS wheels.

## 3. The Quick install design

### 3.1 Items, group, text

* New `Group::Data`, title "Data science toolkit", note "Python with scikit-learn, NumPy and SciPy, and OpenCV for pictures, downloaded from the vendors at your click, run in a sandbox with no network; nothing of it is in the IYAKKI image".
* `Item::DataScience` (id `datasci`, title "Data science toolkit: scikit-learn, NumPy, SciPy"), `needs() = [Item::LinuxApps]` (the sandbox runtime; the item makes its own sandbox, see 3.4).
* `Item::DataVision` (id `datavision`, title "Pictures: OpenCV", `needs() = [Item::DataScience]`), a separate click because it is the biggest download and carries the notices that matter most (FFmpeg LGPL, Intel IPP).
* `licence()` text for both (shown at the click; a "I have read it" confirmation like the firmware packs):
  * datasci: "BSD-3-Clause for scikit-learn, NumPy, SciPy, joblib, threadpoolctl, narwhals, cloudpickle; Python is PSF-2.0. The wheels carry OpenBLAS (BSD), libgfortran and libgomp (GPL-3.0 with the GCC runtime exception) and libquadmath (LGPL-2.1+). IYAKKI does not ship any of it; it downloads the vendors' files, unchanged, and shows their licence files in the folder."
  * datavision: "OpenCV is Apache-2.0. The wheel from PyPI also contains FFmpeg (LGPL-2.1; it reads video files and streams), libvpx, libaom, libdav1d, libavif (BSD), OpenSSL 1.1.1k (old, never used for network here), and on x86_64 Intel IPP, a binary-only library under the Intel Simplified Software License. Nothing of it is shipped by IYAKKI."
* `download_mb()` shown before the click: datasci 64 + 62 = 126, datavision 62 (from the table below; plus 120 for the sandbox base when none exists, said separately). `time_text()` "3 to 8 minutes". A free-space check (2x the unpacked size) before the first byte, in the existing "needs N MB free" style.
* `is_service() = false`, `runs_as_job() = true`, `min_ram_mb() = 1500` (a scikit-learn run measured 159 MB peak resident for 10000x10 `make_classification`; OpenCV import plus a 1080p blur and resize 182 MB; the headroom is for bigger sets).
* Presence: marker file `datasci` in the quickinstall state dir (`mark_done`) **and** the folder check (3.2) so a half-deleted folder shows as "not installed".

### 3.2 Where things live

```
<data>/datasci/
  state.json            versions, wheel names, SHA-256 of what was installed, time
  download/             *.part files while downloading, wheels kept until the install succeeds
  python/               python-build-standalone install_only tree (tests, idle, Tk docs pruned)
  site/                 unpacked wheels (one flat site-packages), plus site/<pkg>.dist-info/licenses/*
  licenses/             Python's, plus the NOTICE text of section 8
  scratch/<run id>/     per-run scratch (3.5), emptied after each run
```
The sandbox `datasci` (Debian 13 slim by default; `linuxapps` creates it) mounts `python/` and `site/` **read-only** at `/opt/datasci/python` and `/opt/datasci/site`. Nothing is written to the base image or to `/usr`.

### 3.3 Pinned table (one Rust `const`, one place to bump)

x86_64 shown; the table has an aarch64 row set (PyPI has cp312 manylinux aarch64 wheels for numpy, scipy, scikit-learn and opencv, checked; their hashes are filled in when the table is built). Hashes shown are the first 16 hex characters, full 64 in the code; every full hash was recomputed from the downloaded file and equals the SHA-256 PyPI publishes (0 mismatches).

| Name | Version | URL (host) | SHA-256 | Size | Licence |
|---|---|---|---|---|---|
| CPython (python-build-standalone, `cpython-3.12.15+20261021-x86_64-unknown-linux-gnu-install_only.tar.gz`) | 3.12.15 | `github.com/astral-sh/python-build-standalone/releases/download/20261021/` (redirects to GitHub's release storage) | `5ff9f0602ce964a5...` (recomputed after download; the release's `SHA256SUMS` is the second source) | 64 MB | PSF-2.0 |
| numpy | 2.5.4 | `files.pythonhosted.org/packages/43/a3/c799c6...` `numpy-2.5.4-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl` | `fbde6962867ee75b...` | 16.7 MB | BSD-3-Clause and others |
| scipy | 1.18.1 | `.../packages/df/64/ff35eb...` `scipy-1.18.1-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl` | `f55fa87b6c612ecd...` | 35.3 MB | BSD-3-Clause |
| scikit-learn | 1.9.1 | `.../packages/8e/57/a50162...` `scikit_learn-1.9.1-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl` | `e5d7b18a5b9dca24...` | 9.2 MB | BSD-3-Clause |
| joblib | 1.6.0 | `.../packages/18/53/840993...` `joblib-1.6.0-py3-none-any.whl` | `3dbbf9f6e4b592a2...` | 0.3 MB | BSD-3-Clause |
| threadpoolctl | 3.7.0 | `.../packages/43/3f/f88a53...` `threadpoolctl-3.7.0-py3-none-any.whl` | `cd8b60b5641b45c6...` | 26 KB | BSD-3-Clause |
| narwhals | 2.27.1 | `.../packages/d1/89/5d4c86...` `narwhals-2.27.1-py3-none-any.whl` | `d057df13f5852b8e...` | 0.5 MB | MIT |
| cloudpickle | 3.1.2 | `.../packages/88/39/799be3...` `cloudpickle-3.1.2-py3-none-any.whl` | `9acb47f6afd73f60...` | 22 KB | BSD-3-Clause |
| opencv-python-headless (item DataVision) | 4.14.0.94 | `.../packages/ae/ce/7f5388...` `opencv_python_headless-4.14.0.94-cp37-abi3-manylinux_2_28_x86_64.whl` | `211e581f5a4670ac...` | 62.0 MB (161 MB unpacked) | Apache-2.0 + LGPL-2.1 (FFmpeg) + Intel (IPP) + others |

The code stores the complete URL (the `/packages/<hash>/` path is content-addressed on PyPI, so the URL cannot be repointed to other bytes without the hash failing). Dependency closure was checked by installing: scikit-learn needs numpy, scipy, joblib, narwhals, threadpoolctl; joblib needs cloudpickle (without it `import sklearn` fails; tested); OpenCV needs numpy.

**Policy hosts, added for the install job only** (the job's host list, like `hosts: &[...]` in `clis.rs` and `gpu_nvidia::host_allowed`; a redirect to any other host is refused): `files.pythonhosted.org`; `github.com` plus its release-asset host (`objects.githubusercontent.com` / `release-assets.githubusercontent.com`) for Python; and, only if the `datasci` sandbox must be created, the hosts `linuxapps_dist.rs` already names for Debian (Docker Hub library). `pypi.org` is **not** needed (URLs are pinned; no index queries). After the job, nothing keeps these hosts open.

### 3.4 Steps (job label "Install Data science toolkit (n/N)", log lines `[n/N] ...` as `macfw.rs` and the firmware packs write them)

1. `[1/9] checking this computer`: Linux apps can run sandboxes (the `linuxapps::Probe`), architecture (x86_64 or aarch64), free space >= 2x unpacked, the item's licence was accepted.
2. `[2/9] Linux sandbox`: if the sandbox `datasci` does not exist, create it with the existing Debian 13 job (hash-checked layers; about 120 MB); say "already there" otherwise. The sandbox is made with the network switch **off**.
3. `[3/9] downloading Python 3.12.15 (64 MB)`: streamed to `download/<name>.part` with `Range` resume (the `macfw.rs`/`camfw.rs` pattern: a stopped download continues from the bytes on disk; the log says "continuing, N MB are on disk").
4. `[4/9] checking Python`: SHA-256 of the finished file against the table **and** the release's `SHA256SUMS`; a mismatch deletes the file, names both hashes, stops.
5. `[5/9] downloading 8 packages (62 MB)`: each wheel the same way, one line per file.
6. `[6/9] checking the packages`: SHA-256 of each wheel against the table; only then is any wheel opened. Also checks: the file is a `.whl` (zip), the name's tags match the table, **no wheel that is not in the table is ever fetched**.
7. `[7/9] unpacking`: Python: our tar.gz reader (`archive.rs`) into `python/` with the strict path rules (no `..`, no absolute, no links outside, size caps as in `rootfs.rs`), then prune `lib/python3.12/test`, `idlelib`, `tkinter`, `ensurepip`, docs. Wheels: our zip reader into `site/`, with the same rules; `*.data/scripts`, `*.pth`, `sitecustomize`, `usercustomize` and any `console_scripts` are **not** extracted or run. **No pip, no setup.py, no hooks, no code runs during install.**
8. `[8/9] testing`: runs, in the sandbox with network off, a fixed self-test script (embedded in IYAKKI, not from a file the person can change): imports each package, prints versions, fits a 300-row `RandomForestClassifier`, checks the score is above 0.9, and for OpenCV resizes a generated image. Pass line: "scikit-learn 1.9.1 fits a small model in 0.6 s". On failure the folder is kept for the log and the item shows "failed: <last line>" with Remove available.
9. `[9/9] done`: writes `state.json` and the marker, deletes `download/`, tells where the licences are (`<data>/datasci/licenses/`).

DataVision is the same list with steps 3 to 8 for one wheel and an OpenCV self-test (`cv2.getBuildInformation()` runs, a blur and a resize, `cv2.imencode` of a PNG).

### 3.5 Run model (what `python_run` uses; section 4)

`iyakki linuxapp-init ... --net off --mem-mb 1024 --cpu-pct 200 --pids 64 --tmp-mb 512 --seccomp on --bind-ro <data>/datasci/python /opt/datasci/python --bind-ro <data>/datasci/site /opt/datasci/site --bind-rw <data>/datasci/scratch/<run> /work --cwd /work --env PYTHONPATH=/opt/datasci/site --env PYTHONHOME=/opt/datasci/python --env OMP_NUM_THREADS=2 --env OPENBLAS_NUM_THREADS=2 --env PYTHONDONTWRITEBYTECODE=1 --env HOME=/work -- /opt/datasci/python/bin/python3 -I /work/script.py` (flags as documented in `LINUX-APPS.md` section 9; the exact flag set is checked against `linuxapps_init.rs` when built), under `nice`, with a wall-clock limit enforced by the caller (kill the process group).

### 3.6 Find, verify, remove, reset, offline

* **How `python_run` finds it:** `datasci::installed(data) -> Option<Toolkit>` reads `state.json`, checks the folder exists and the **size and mtime of each recorded wheel's main file** (cheap; a full re-hash is the Settings "Verify toolkit" button), and returns the paths. If anything is off: treated as not installed.
* **Upgrade:** the pin table carries a version; a newer app with a newer table shows "Upgrade" in the same item (existing `check_upgrades` pattern) and installs side by side then swaps (`datasci.new` renamed over `datasci` in one step).
* **Remove / reset:** "Remove" deletes `python/`, `site/`, `licenses/`, `state.json`, marker; keeps the `datasci` sandbox (one line says it can be deleted on the Linux apps page). "Delete download" removes `download/` (as macfw's leftovers). Data tab files and Media outputs are never touched.
* **Offline:** a run needs no network (it has none by design). Install needs the pinned hosts; offline it stops at step 3 or 5 with "cannot reach files.pythonhosted.org (no network)" and keeps what it has, so a retry resumes. A toolkit already installed works with the computer offline.
* **Mirror / air-gapped:** a "from a folder or stick" source like the firmware packs (same hashes checked) is a cheap later extra.

## 4. How the agent uses it

### 4.1 Tool `python_run`

```
python_run { script: string (<= 20000 chars), inputs: [{name, handle}] (Data tab handles or Files paths, <= 8),
             outputs: [{file, kind: "table"|"image"|"file"}], timeout_s (1..300, default 60), memory_mb (<= 2048, default 1024) }
```
* **Approval card** every time (like `terminal` today): shows the script text in full, the inputs by name, the limits, "no network, no access to your files except the inputs and the scratch folder", and **Allow once / Allow for this chat** is not offered for the first run of a chat.
* **Inputs:** each input is copied (CSV, up to a size cap) to `/work/in/<name>` read-only; the script reads `in/x.csv` with `numpy.genfromtxt`/`csv` (pandas is not in v1; the item can add it later). Images are copied to `/work/in/`.
* **Outputs:** only the files listed under `outputs` are collected from `/work/out/`, size-capped (50 MB each, 20 files). `kind: table`: a CSV (checked: header row, consistent column count, UTF-8) is registered through the existing `data_create` path, so it appears in the Data tab like any other dataset. `kind: image`: copied to the Media/Files folder the person chose (default `Pictures/ai-output/`, never overwriting) and the Media tab lists it. Anything else stays in scratch and is deleted.
* **Result text to the model:** exit status, the first 4000 characters of stdout, 2000 of stderr, output names with row counts or sizes, time and peak memory.
* **Limits:** wall clock (kill group), memory (cgroup, `--mem-mb`), CPU (2 threads, `nice 10`; `cpu-pct 200`), 64 processes, 512 MB scratch, **network off**, seccomp on, a fresh scratch per run, 4 runs per hour per person unless an administrator changes it (audit-visible).
* **Not installed (or the Linux apps runtime missing): the agent's plain answers.** Nothing is attempted, no network is touched:
  * toolkit missing: "I can do that with Python, but the Data science toolkit is not installed on this computer. Open Store, then Setup, then Data science toolkit to download it (about 126 MB, checked and kept in your data folder). Until then I can make a small test set with the built-in generator instead. Shall I do that?"
  * OpenCV missing: "Picture processing needs the Pictures: OpenCV add-on (about 62 MB). It is in Store, Setup, next to the Data science toolkit."
  * sandbox unavailable on this kernel: the `Probe` message that Linux apps already shows (which kernel options are missing).
  * offline during install: "The download needs the internet once; I could not reach files.pythonhosted.org. Nothing was changed."

### 4.2 Test-set recipes (the agent's system hint lists these; every script fixes `random_state`)

* **Tabular classification:** `make_classification(n_samples, n_features, n_informative, n_classes, weights, flip_y, class_sep, random_state)` -> columns `f1..fN,label`; `numpy.savetxt` or the `csv` module to `out/set.csv`. **Regression:** `make_regression(..., noise=..)` with `n_informative`, `bias`. **Clusters:** `make_blobs`, `make_moons`, `make_circles`.
* **Messy data:** after generation, blank out a chosen fraction at random per column (missing values), add duplicates, shift a column's scale, insert outliers (`x * 20` for 1%), mixed-case categories and date strings in two formats, so a cleaning flow can be tested.
* **Time series:** `t = arange(n)`, trend + seasonal sine (period 7 or 24) + `normal(0, s)` noise + a few spikes; timestamps from `datetime` at a chosen step; a second column with a lag of the first.
* **Text:** words drawn from a word list with a seeded `choice` (a small list embedded in the script or a file from the Data tab), sentences of random length, a label tied to keyword presence, optional typos.
* **Relational:** two tables with a shared key (`customers.csv`, `orders.csv`) with a chosen share of orphan keys, for join tests.
* **Models:** `train_test_split`, `RandomForestClassifier`/`LogisticRegression`/`KMeans`/`LinearRegression` on an input table; outputs `predictions.csv` and `metrics.csv` (a table), so the result is in the Data tab and can be charted in DASH.
* **OpenCV (Media):** read `in/photo.jpg`; `resize`, `GaussianBlur`, `cvtColor` to grey, `Canny` edges, `threshold`, `equalizeHist`, rotate/crop, contour counting, face detection with the Haar cascade files the 4.14 wheel carries (`cv2/data/haarcascade_*.xml`, Apache-2.0; `cv2.data.haarcascades`), people with `HOGDescriptor`, QR codes with `QRCodeDetector` (no network), image montage/contact sheet, frame extraction from a video the person points at (FFmpeg inside the wheel; no other codec work). Outputs `kind: image` to the chosen Media/Files folder.
### 4.3 OpenCV without a screen, and the camera

**No GPU, no screen.** Use `opencv-python-headless` only. It has no GUI back end: `imshow`, `waitKey` windows and `highgui` do not exist and are never called, so no X server, no virtual framebuffer and no Qt is needed (the non-headless wheel is excluded in 1.1). Results leave as files (`imwrite`, CSV), never as windows. Everything below is CPU-only. **CUDA/OpenCL-accelerated or GPU builds of OpenCV, cuDNN and GPU inference are out of scope** (the PyPI wheels have no CUDA; a CUDA build would be our own build and the NVIDIA licence path in `GPU-K3S.md`).

**Recipes that exist in the pinned wheel (checked by importing it):** Haar cascades (frontal face, eyes, bodies; 4.14 only), `HOGDescriptor` people detector (4.14 only), motion/background subtraction (`createBackgroundSubtractorMOG2`/`KNN`), `goodFeaturesToTrack` + `calcOpticalFlowPyrLK`, contours and morphology, `QRCodeDetector`, `barcode_BarcodeDetector`, ArUco, and `cv2.dnn` / `FaceDetectorYN` for small **ONNX** models the person supplies (for example the YuNet face detector; its licence is the model's own and is not part of the toolkit; no model is downloaded by the toolkit).

**Expected speed (honest).** Measured on 2026-10-25 with the pinned 4.14.0.94 wheel on a fast 2023 laptop CPU (AMD Ryzen 9 7940HS, OpenCV limited to 2 threads, 640x480, a synthetic blurred image, no real faces): HOG people detector **28 frames per second**; MOG2 motion mask **about 570 fps**; blur + Canny **about 6000 fps**; Haar frontal face scan **369 to 565 fps on an image with no face-like regions, which flatters it** (a real scene with faces is much slower, plausibly 40 to 150 fps; not measured). **Not measured:** `cv2.dnn` with an ONNX detector (no model was available), anything on the Mac mini or a MacBook, anything through the camera. Older dual-core machines (the 2014 Mac mini) should be expected several times slower than the numbers above (an estimate, not a measurement): plan on about 5 to 10 fps for HOG and 20 to 60 fps for motion at 640x480 there, and use smaller frames or every Nth frame.

**Camera from a run (design, approved by the owner; nothing built).**
* `cv2.VideoCapture(<index or /dev/videoN>, cv2.CAP_V4L2)` reads the V4L2 node directly. The run gets **only that one device node** (bind-mounted into the sandbox) and only when the person approves; the sandbox otherwise has no `/dev/video*`.
* **Approval card** names the camera (the name the CAMERA tile shows, for example "Apple Facetime HD" or the webcam's card name), the script in full, the limits, and says "the camera will be on while this runs". A separate approval from the one for files; "Allow once" only, never "for this chat".
* **One reader at a time.** The CAMERA tile (and the assistant's photo tools, `agentcam.rs`) must release the device first. If it is busy, the run does not start and the agent says: "The camera is in use (the CAMERA app or another run holds it). Close it and ask me again." The same text style as the existing camera tools use for `EBUSY`.
* **Visible indicator.** While a run holds the camera the shell shows the same blinking **CAMERA IS ON** mark the QR-scan card uses (`MAC-CAMERA.md`), plus the run's name and a Stop button; the mark is tied to the run's lifetime in the supervisor, not to the script, so a script cannot hide it.
* **Where frames go.** Frames live in the run's scratch folder (`/work`, a tmpfs-like folder emptied after the run). They leave only as named outputs through `data_create` (tables: counts, boxes, timestamps) or as images saved to the person's chosen Media folder; the chat gets text, not pictures, unless the person's approval allowed saving. No network inside the run, so nothing can be uploaded.
* **Limits.** Wall clock (default 60 s, max 300 s), a frame cap (default 1800, max 18000), resolution as the camera offers, memory and CPU limits as in 3.5, scratch 512 MB.
* **Audit line** (no images, no script text): time, person, camera name, script SHA-256, duration, frames read, outputs (names and sizes), exit status, "camera released".
* **Fallback design if binding a device node into the sandbox is not feasible** (the seccomp list, device cgroup and `linuxapp-init` bind rules need checking, section 9): the shell's own capture code (`crates/iyakki-shell/src/camera/v4l2.rs`, no OpenCV) reads the camera and writes frames as numbered JPEGs into the run's scratch folder at a capped rate; the script reads them with `cv2.VideoCapture("/work/frames/%06d.jpg")` or `imread`. The sandbox then never sees the device, the indicator and the single-reader rule are enforced by our own code, and only the format conversion costs a little CPU. This is the safer default if the device path has any doubt.
* **Video files and streams.** `cv2.VideoCapture("in/clip.mp4")` and `VideoWriter` use the FFmpeg inside the wheel (LGPL-2.1, section 1.1), including its H.264/AAC/VP9/AV1 decoders. Network URLs fail by design (no network). The notice text and the item text name FFmpeg and the patent remark (1.2, 8).
* **Hardware.** **USB webcams (UVC, driver `uvcvideo`) are the supported path.** The V4L2 pixel formats seen so far are MJPEG and YUYV, which OpenCV converts. **The built-in camera of a T2 MacBook is unverified for this work and must not be claimed to work:** `MAC-HARDWARE.md` and `MAC-CAMERA.md` cover the non-T2 FaceTime HD (PCI `14e4:1570`, out-of-tree `facetimehd` driver, Apple's firmware extracted on the Mac, YUYV only, "not yet tried on a MacBook" in those notes) and the camera is "not verified" on the T2 models; whether a T2 MacBook exposes a `/dev/video*` node in our image at all is an open item. What to check, in order: (1) does the node exist and does the CAMERA tile open it (`iyakki-shell/src/camera/v4l2.rs` is the same path); (2) its formats and sizes (YUYV only for `facetimehd`); (3) `cv2.VideoCapture(path, cv2.CAP_V4L2)` returns frames for 30 s without a stall; (4) the sandbox sees the node and a second reader gets `EBUSY`; (5) CPU use at 640x480 with a trivial script; (6) LED and the indicator behave. Until then, the plan's real-hardware camera test uses a USB webcam on a PC or Mac mini.

* **Where it shows:** CSV outputs: Data tab (new dataset, name = file name, source "python_run"); images: Media tab and the Files tab in the output folder; the chat gets the summary text and the names.

## 5. Security gate for v1.0.0

Items marked "test" must have a test with fakes; "real" needs the one real run on a device.

| Area | Requirement | Check |
|---|---|---|
| Supply chain | exact URLs and full SHA-256 in one table; wheels only (no `.tar.gz`, no sdist, no build); hash verified **before** any wheel is opened; unknown file in the job = refused; redirect off the host list = refused | test: wrong hash, wrong host, extra file, redirect |
| No install-time code | no pip, no `setup.py`, no `.pth`/`sitecustomize`/entry-point scripts are extracted or run | test: a wheel with a `.pth` and a script is unpacked without them |
| Unpack safety | zip/tar reader refuses `..`, absolute paths, links leaving the root, device/set-id files, size and count bombs | test (same reader tests as `rootfs.rs`) |
| Sandbox | `linuxapp-init`: namespaces, seccomp on, no network, read-only toolkit mounts, writable scratch only, memory/pids/CPU/time limits | test: the exact argv; real: a script that opens a socket fails, writes outside `/work` fails, allocates beyond the limit is killed |
| Approval | every `python_run` shows the whole script and the inputs; no silent run; in unattended modes (tiers 0 to 4) the call is **refused** unless the owner has allowed it for that automation | test |
| Inputs and outputs | inputs only what the tool call names, read-only; outputs only named files, size-capped, types checked; no overwrite outside the chosen folders | test |
| Camera | device node bound only on a per-run approval naming the camera and showing the script; one reader (busy = refused with a plain message); indicator tied to the run; no network; time and frame caps; frames only in scratch unless written out by a named output; audit line has camera name, script hash, duration, frame count, no images | test: fake opener (busy, refused, released on kill); real: a USB webcam run, a second reader fails, the indicator shows and clears |
| Audit | one line per run: time, person, script SHA-256, input names, output names and sizes, exit status, duration, peak memory, limits. **No script text, no file contents, no environment** in the line (the script is shown on the card and kept in the chat as it was); install: item, versions, hashes, hosts used | test: no secret strings (a token inside a script) reach the audit file |
| What is refused | network access (any `socket`/`urllib` use just fails; the tool does not offer the switch); `pip install` from a script (no network, no pip); shell-outs beyond what seccomp and the empty root allow; reading `/home`, the vault, other people's data; scripts over the size cap; more than the limits; running when the toolkit failed its self-test | test + real |
| Licence | the notice text (section 8) is shown at the click, kept in `<data>/datasci/licenses/`, listed in `LICENSING.md`; `deny.toml` is unaffected (no Rust dependency is added) | review |
| Update path | CVE: bump the table, release; a Settings line tells when the installed pins are older than the app's table. OpenCV's OpenSSL 1.1.1k is never used (no network inside) and the item text says so | review |

## 6. Implementation plan (small steps, each testable with fakes)

| # | Step | Test | Size |
|---|---|---|---|
| 0 | One real check on a dev PC: create the Debian sandbox, mount the unpacked tree read-only, run the self-test; confirm `libstdc++`/`libgcc_s`/`libz` exist in `debian:*-slim` and glibc >= 2.28; record peak memory and time | manual, notes in the doc | S (half a day) |
| 1 | `datasci.rs` pin table (data only) + `Item::DataScience`/`DataVision`, `Group::Data`, texts, `download_mb`, licences; the existing quickinstall tests that count items updated | table tests: URLs on the allowed hosts, 64-hex hashes, versions match file names | S |
| 2 | Download with resume + verify (reuse the `macfw.rs` fetch helper behind a trait) | fake server with `Range`; wrong hash; redirect to another host | M |
| 3 | Safe unpack (tar.gz for Python, zip for wheels) with the exclusions and pruning | malicious archives; `.pth` and scripts skipped | M |
| 4 | `StepEnv` steps 1 to 9 in `run_step`, `[n/N]` log lines, markers, remove/reset, upgrade flag | the fake `StepEnv`; interrupted job resumes | M |
| 5 | Sandbox: create/reuse `datasci` (network off), exec argv builder for the run model (3.5) | argv test with the fake `Runner`; limits present | M |
| 6 | Self-test script + result parsing; "Verify toolkit" | fake runner returns pass/fail text | S |
| 7 | `python_run` tool: schema, approval card text, inputs copy-in, outputs collect, caps, audit line, not-installed messages | fakes for everything; a table output lands in the Data service | L |
| 7b | Camera in a run: shared single-reader lock with the CAMERA tile and `agentcam.rs`, indicator, approval card text, frame cap, audit; first the scratch-folder JPEG feeder (fallback design), then the device-node bind if the sandbox allows it | fake `Opener` (busy, release on kill, frame cap); one real run with a USB webcam | M |
| 8 | Prompt hint + recipes (section 4.2); a small Rust `data_generate` (seeded generator) as the no-Python path | deterministic outputs for a seed | M |
| 9 | UI: Store -> Setup rows, licence click-through, size before click, progress, Remove | shell tests like other items | M |
| 10 | Real device run on the Mac mini and a PC, then docs (`DATA.md`, `USER-GUIDE.md`, `FEATURES.md`, `LICENSING.md`) | owner | S |

Total: about 3 to 4 working weeks for steps 1 to 9 (about 4 to 5 with 7b) for one engineer. Build order: 1, 3, 2, 4, 5 (item installs and self-tests), then 7, then 8 (so test sets can be made without Python), then 9.

**Risks:** (1) the Debian slim image may lack a library a wheel needs (step 0 finds out; fallback: `apt install libstdc++6` at install time in a network-on window, hosts added to that step only); (2) the kernel on some Macs may lack namespaces (the existing Linux apps probe says so; the toolkit is then simply unavailable); (3) BLAS/OpenMP thread storms: caps in 3.5, verified in step 0; (4) 600 MB disk on small devices: disk size shown before the click, Remove always available; (5) a wheel's bundled library gets a CVE (OpenCV's OpenSSL, FFmpeg): bump pins, the no-network sandbox limits the exposure to crafted input files (a hostile image or video decoded by FFmpeg/libpng is the real exposure; the sandbox contains it, and the card says which files are read); (6) wheel and Python pin drift: one test fails when the table's Python tag and the wheel tags disagree.

## 7. Decisions for the owner

1. **Ship nothing, offer as click-to-download** (decided, option (a)). Confirm the two items (DataScience, DataVision) or one.
2. **glibc sandbox (Debian) for running**, since scikit-learn and OpenCV have no musllinux wheels; musl-only is possible for NumPy/SciPy/pandas/Pillow but not for the two asked-for libraries. Dedicated sandbox `datasci` (+120 MB, cleaner removal) or reuse `debian`?
3. **Versions to pin:** Python 3.12.15 (python-build-standalone 20261021), numpy 2.5.4, scipy 1.18.1, scikit-learn 1.9.1, joblib 1.6.0, threadpoolctl 3.7.0, narwhals 2.27.1, cloudpickle 3.1.2, opencv-python-headless 4.14.0.94. Python 3.12 (not 3.13/3.14) because every wheel above has a cp312 build and it is the version the existing `python` catalogue item installs.
4. **OpenCV with Intel IPP and FFmpeg inside, as downloaded** (acceptable for a click-download with notices) versus building our own OpenCV without them (weeks, and then it would have to ship from us). Recommendation: accept the download, revisit only if the notice text worries counsel.
5. **pandas and matplotlib:** leave out of v1 (Data tab and DASH cover them)?
6. **Unattended modes (tiers 0 to 4):** refuse `python_run` unless the owner whitelists a script hash, or never allow it?
7. **Run limits and quota** (1024 MB, 60 s default, 300 s max, 4 runs/hour).
8. **Build the Rust `data_generate` first** (small, no licence question, works offline and in tier 1) even if the toolkit is delayed?
9. **OpenCV 4.14 or 5.0:** pin 4.14.0.94 (has Haar and HOG, which the camera recipes use) or 5.0.0.93 (newer; no cascade classifier, no HOG; ONNX/`dnn` and `FaceDetectorYN` only). Recommendation: 4.14.0.94 now, revisit when 5.x regains them.
10. **Camera in a run:** device node (needs the sandbox to bind a V4L2 node) or the scratch-folder frame feeder (safer, our code reads the camera). Recommendation: build the feeder first; USB webcams only until the T2 built-in camera is tested.
11. **No lawyer reviewed this** (same standing note as the firmware policy); ask for a one-time review of the notice text and the IPP point before 1.0.0, or accept the risk like the firmware packs.

## 8. Proposed `LICENSING.md` note (not applied; for the owner to place as a new numbered item)

> **Data science toolkit: Python, scikit-learn, NumPy, SciPy, OpenCV, downloaded by the person (Quick install; `docs/development/DATA-SCIENCE-ADDON-PLAN.md`).** Nothing of this is in any IYAKKI image or in this repository. At the person's click IYAKKI downloads, from the vendors' hosts only and checked against SHA-256 values pinned in IYAKKI, the python-build-standalone CPython 3.12 build (PSF-2.0; github.com/astral-sh) and these wheels from PyPI (files.pythonhosted.org): numpy (BSD-3-Clause and others), scipy (BSD-3-Clause), scikit-learn (BSD-3-Clause), joblib, threadpoolctl, cloudpickle (BSD-3-Clause), narwhals (MIT) and, as a separate click, opencv-python-headless (OpenCV is Apache-2.0; the packaging is MIT). The wheels contain third-party libraries under their own licences, which stay with them in `<data>/datasci/licenses/`: OpenBLAS (BSD-3-Clause) and LAPACK (BSD-3-Clause-Open-MPI); libgfortran and libgomp (GPL-3.0-or-later with the GCC Runtime Library Exception); libquadmath (LGPL-2.1-or-later); in the OpenCV wheel FFmpeg (LGPL-2.1, built without GPL options; it reads video files and streams for OpenCV's `VideoCapture`), libvpx, libaom and libavif (BSD), OpenSSL 1.1.1k (OpenSSL licence; never used, the toolkit runs without network), and, on x86_64, Intel IPP (a binary-only library under the Intel Simplified Software License, redistributed unmodified by the OpenCV project). IYAKKI does not ship, link, modify or redistribute any of them; it fetches the vendors' unchanged files into the data folder, unpacks them with its own reader (no pip, no install scripts) and runs them as separate programs in a no-network Linux sandbox (`BOUNDARY.md`). The licence notice above is shown before the download. H.264, AAC and other patent-licensed formats that FFmpeg can decode are the person's choice, as with the ffmpeg add-on (item 10). Check the licences before using the results commercially; scripts and data produced by the person belong to the person.

## 9. What was not verified, and how it was checked or why not

* **Verified:** PyPI versions, file lists (musllinux/manylinux), licence metadata, bundled licence files, `*.libs` contents, FFmpeg's configure line in `libavcodec` (no `--enable-gpl`), Intel IPP statement in OpenCV's `LICENSE-3RD-PARTY.txt`, SHA-256 of all downloaded wheels against PyPI's JSON, that Python 3.12.15 from python-build-standalone (glibc, x86_64; SHA-256 recomputed) imports numpy, scipy, scikit-learn, OpenCV, Pillow from plainly unzipped wheels and fits a random forest (on a glibc development PC, **not** inside an IYAKKI sandbox), that `import sklearn` needs cloudpickle, and peak memory/time (159 MB and 0.7 s for `make_classification` 10000x10 with one BLAS thread; 182 MB total after an OpenCV 1080p blur and resize).
* **Camera and speed:** the camera design was not run at all (no camera was touched; no sandbox was used); `cv2.VideoCapture` on V4L2 was not tested; the T2 MacBook camera is unverified (state above). The frame rates in 4.3 are from one run on a development laptop with a synthetic image; DNN speed and every slower machine are estimates or absent. The 4.14.0.94 wheel was audited for the same points as 5.0.0.93 (FFmpeg configure line, IPP notice present, OpenSSL 1.1.1k, libs listed) and its hash recomputed, but its dependency pins and the aarch64 wheel were not opened.
* **Not verified:** (1) anything on a real IYAKKI device or inside a Linux apps sandbox (step 0 of the plan); presence of `libstdc++6` and `libgcc-s1` in `debian:13-slim`; (2) the aarch64 wheels' licence contents and hashes (their existence was checked in the file list; their `LICENSE-3RD-PARTY.txt` may differ, e.g. no IPP on aarch64); (3) the python-build-standalone bundled-library licence list (it is in the archive's `licenses/` or `python/PYTHON.json`; not read); the digest of the musl and aarch64 builds were not recomputed; the x86_64 glibc digest was; (4) the Intel IPP text says "2021 Update 10" while OpenCV's build info reports IPP 2026.0.0; the licence text applies, version mismatch left to counsel; (5) matplotlib and pandas licence files were listed, not read line by line; Pillow's bundled-library licence text was skimmed, not audited line by line; (6) PyPI file lists change as new releases appear: the table must be rebuilt (script in step 1) at implementation time; (7) no legal review; (8) `linuxapp-init` flag names in 3.5 are from `LINUX-APPS.md` and were not run.
