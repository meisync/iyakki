#!/bin/sh
# build-wl-pack.sh - build Broadcom's proprietary `wl` Wi-Fi driver for the IYAKKI Mac image and wrap it in a signed
# IYAKKI driver pack, for YOUR OWN Mac (Mac mini 2014 and other BCM4360 / BCM43a0-family cards).
#
# Run it on a Linux build PC, never on the Mac. IYAKKI does not ship, mirror or redistribute Broadcom's driver
# (licence: Broadcom's own, not GPL, binary-only part): this script downloads it from Broadcom, builds it for the exact
# kernel of your image, and puts the result in a pack that only your own publisher key signs. Nothing it makes is
# meant to be committed or shared: dist/drivers/out/ and *.ko are in .gitignore.
#
# Usage:
#   sh dist/drivers/build-wl-pack.sh --accept-broadcom-licence [options]
#
# Options (all optional):
#   --accept-broadcom-licence   you have read Broadcom's licence, which the script prints, and accept it. Required.
#   --kernel-tree DIR           the kernel build tree of the Mac image
#                               (default: target/buildroot-mac/build/linux-<version>, the newest one)
#   --toolchain DIR             directory with the image's cross compiler (default: <tree>/../../host/bin)
#   --cross-compile PREFIX      compiler prefix, e.g. x86_64-buildroot-linux-musl- (default: found in --toolchain)
#   --prepare-in-copy           the tree is not prepared for out-of-tree modules: run `make modules_prepare` in a COPY
#   --source FILE               Broadcom's tarball, already downloaded (offline use); its sha256 must match the pin
#   --patches DIR               community kernel-compat patches instead of the pinned Debian set: a folder with a
#                               `series` file (or *.patch files, applied in name order); paths as in Debian's
#                               debian/patches (a/amd64/...) or plain (a/src/...)
#   --out DIR                   where the pack goes (default dist/drivers/out)
#   --key FILE                  your publisher signing key (default ~/.config/iyakki/publisher/wl-owner.key; made if absent)
#   --version X.Y.Z             pack version (default 1.<yymmdd>.<hhmm>)
#   --iyakki FILE               the iyakki program (default: PATH, then target/release, then target/debug)
#   --work DIR                  scratch folder (default: a new folder under $TMPDIR or /tmp); kept with --keep
#   --keep                      keep the scratch folder (the built wl.ko stays in it; never commit it)
#   --jobs N                    parallel compile jobs (default: number of CPUs)
#   -h, --help                  this text
#
# Exit codes: 0 done, 2 usage, 3 licence not accepted, 4 a tool is missing, 5 kernel tree problem, 6 download or
# checksum problem, 7 a patch does not apply, 8 compile error, 9 the module does not match the kernel, 10 packaging.
set -u

# ---- pinned inputs --------------------------------------------------------------------------------------------
# Broadcom's official download of the hybrid 802.11 Linux STA driver 6.30.223.271 (file dated 2025-10-15 on their server).
SRC_URL="https://docs.broadcom.com/docs-and-downloads/docs/linux_sta/hybrid-v35_64-nodebug-pcoem-6_30_223_271.tar.gz"
SRC_SHA256="5f79774d5beec8f7636b59c0fb07a03108eef1e3fd3245638b20858c714144be"
SRC_NAME="hybrid-v35_64-nodebug-pcoem-6_30_223_271.tar.gz"
# Debian's kernel-compatibility patches for that source (package broadcom-sta 6.30.223.271-26, Debian 13 "trixie",
# which ships kernel 6.12). The same patches build with kernels 6.1 to 6.12.
PATCH_URL="https://deb.debian.org/debian/pool/non-free/b/broadcom-sta/broadcom-sta_6.30.223.271-26.debian.tar.xz"
PATCH_SHA256="858ab161a64782b4c1d25b6e28e5e16b06e23e5bea3b9d17c62c90145e1afd1d"
PATCH_NAME="broadcom-sta_6.30.223.271-26.debian.tar.xz"

PACK_NAME="mac-wifi-bcm4360"     # the Store's built-in entry this pack belongs to (do not change)
PUB_ID="wl-owner"

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/../.." && pwd)

die() { code=$1; shift; printf '\nERROR: %s\n' "$*" >&2; exit "$code"; }
say() { printf '%s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }

usage() { sed -n '2,/^set -u/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; }

# ---- arguments ------------------------------------------------------------------------------------------------
accept=0; tree=""; toolchain=""; cross=""; prep=0; source_file=""; patches_dir=""; out="$here/out"
keyfile=""; version=""; iyakki=""; work=""; keep=0; jobs=""
need_val() { [ $# -ge 2 ] || die 2 "$1 needs a value (see --help)"; }
while [ $# -gt 0 ]; do
  case $1 in
    --accept-broadcom-licence) accept=1 ;;
    --kernel-tree) need_val "$@"; tree=$2; shift ;;
    --toolchain) need_val "$@"; toolchain=$2; shift ;;
    --cross-compile) need_val "$@"; cross=$2; shift ;;
    --prepare-in-copy) prep=1 ;;
    --source) need_val "$@"; source_file=$2; shift ;;
    --patches) need_val "$@"; patches_dir=$2; shift ;;
    --out) need_val "$@"; out=$2; shift ;;
    --key) need_val "$@"; keyfile=$2; shift ;;
    --version) need_val "$@"; version=$2; shift ;;
    --iyakki) need_val "$@"; iyakki=$2; shift ;;
    --work) need_val "$@"; work=$2; shift ;;
    --keep) keep=1 ;;
    --jobs) need_val "$@"; jobs=$2; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die 2 "unknown option $1 (see --help)" ;;
  esac
  shift
done

# ---- tools ----------------------------------------------------------------------------------------------------
step "Checking the tools on this PC"
missing=""
for t in make patch tar sha256sum awk sed grep find cp mkdir base64 tr date xz gzip; do
  command -v "$t" >/dev/null 2>&1 || missing="$missing $t"
done
fetch=""
if command -v curl >/dev/null 2>&1; then fetch=curl; elif command -v wget >/dev/null 2>&1; then fetch=wget; fi
[ -n "$missing" ] && die 4 "missing tools:$missing. On Debian or Ubuntu: sudo apt install build-essential patch xz-utils coreutils curl"
if [ -z "$fetch" ] && { [ -z "$source_file" ] || [ -z "$patches_dir" ]; }; then
  die 4 "neither curl nor wget is installed, and --source and --patches were not both given"
fi
command -v gcc >/dev/null 2>&1 || command -v cc >/dev/null 2>&1 || die 4 "no host C compiler (the kernel build tools need one). Install build-essential."
if [ "$(uname -s)" != "Linux" ]; then die 4 "this script must run on a Linux PC (the kernel's build system needs it)"; fi
case $(uname -m) in x86_64) : ;; *) say "note: this PC is $(uname -m); the image kernel is x86_64 and is built with its own cross compiler" ;; esac
[ -r /sys/class/dmi/id/sys_vendor ] && case $(cat /sys/class/dmi/id/sys_vendor 2>/dev/null) in Apple*) die 4 "this looks like a Mac. Run this script on a Linux build PC, not on the Mac you are building for." ;; esac
[ -n "$jobs" ] || jobs=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 2)

# ---- the iyakki program (signing, module checks) --------------------------------------------------------------
if [ -z "$iyakki" ]; then
  # the first candidate that knows the driver-pack commands (a stale release build may not)
  for c in "$(command -v iyakki 2>/dev/null)" "$repo/target/release/iyakki" "$repo/target/debug/iyakki"; do
    if [ -n "$c" ] && [ -x "$c" ] && "$c" pkg help 2>/dev/null | grep -q 'inspect-module'; then iyakki=$c; break; fi
  done
fi
[ -n "$iyakki" ] && [ -x "$iyakki" ] || die 4 "no up-to-date iyakki program was found (it needs 'pkg inspect-module', 'pkg driver-check', 'pkg keygen' and 'pkg build').
  Build it once: cargo build --release -p iyakki-core   (or pass --iyakki FILE)"
"$iyakki" pkg help 2>/dev/null | grep -q 'inspect-module' || die 4 "$iyakki is too old (it has no 'pkg inspect-module'). Rebuild it: cargo build --release -p iyakki-core"
say "iyakki:  $iyakki"

# ---- the kernel tree --------------------------------------------------------------------------------------------
step "Checking the kernel build tree of the Mac image"
if [ -z "$tree" ]; then
  for c in $(ls -d "$repo"/target/buildroot-mac/build/linux-[0-9]* 2>/dev/null | sort -V -r); do
    case $c in *headers*) continue ;; esac
    [ -f "$c/.config" ] && { tree=$c; break; }
  done
fi
[ -n "$tree" ] || die 5 "no kernel build tree found under target/buildroot-mac/build/. Build the Mac image once (dist/buildroot/build.sh mac) or pass --kernel-tree DIR."
treeabs=$(CDPATH= cd -- "$tree" 2>/dev/null && pwd) || die 5 "the kernel tree '$tree' is not a folder. Build the Mac image first (dist/buildroot/build.sh mac) or pass --kernel-tree DIR."
tree=$treeabs
say "tree:    $tree"
bad=""
for f in .config Module.symvers include/generated/autoconf.h include/generated/utsrelease.h include/config/auto.conf scripts/mod/modpost Makefile; do
  [ -e "$tree/$f" ] || bad="$bad $f"
done
grep -q '^CONFIG_OBJTOOL=y' "$tree/.config" 2>/dev/null && [ ! -x "$tree/tools/objtool/objtool" ] && bad="$bad tools/objtool/objtool"
grep -q '^CONFIG_MODULES=y' "$tree/.config" 2>/dev/null || die 5 "the kernel in $tree is built without CONFIG_MODULES, so no module can ever load."
rel=$(sed -n 's/^#define UTS_RELEASE "\(.*\)"/\1/p' "$tree/include/generated/utsrelease.h" 2>/dev/null)
[ -n "$rel" ] || bad="$bad (UTS_RELEASE)"
if [ -n "$bad" ] && [ "$prep" -eq 0 ]; then
  die 5 "the tree is not ready for out-of-tree modules; missing:$bad
  The tree must have been built (a finished Mac image build leaves it so). To prepare it yourself:
      make -C $tree ARCH=x86_64 CROSS_COMPILE=<prefix> modules_prepare
  which writes into the tree (and wants the same compiler as the image). Safer: let this script do it in a COPY:
      sh $0 --prepare-in-copy ...   (copies the tree, runs modules_prepare there; $tree is not touched)"
fi
if grep -q '^CONFIG_MODULE_SIG_FORCE=y' "$tree/.config"; then
  die 5 "the image kernel enforces module signatures (CONFIG_MODULE_SIG_FORCE=y), so an unsigned wl.ko can never load. Change the kernel config and rebuild the image."
fi
say "kernel:  $rel"
cc_text=$(sed -n 's/^CONFIG_CC_VERSION_TEXT="\(.*\)"/\1/p' "$tree/.config")
say "built with: $cc_text"

# compiler: the image's own cross compiler (so the compiler matches what built the kernel)
if [ -z "$toolchain" ]; then toolchain=$(CDPATH= cd -- "$tree/../../host/bin" 2>/dev/null && pwd || true); fi
if [ -z "$cross" ] && [ -n "$toolchain" ] && [ -d "$toolchain" ]; then
  g=$(ls "$toolchain"/*-gcc 2>/dev/null | head -1)
  [ -n "$g" ] && cross=$(basename "$g" | sed 's/gcc$//')
fi
if [ -n "$cross" ] && [ -n "$toolchain" ] && [ -d "$toolchain" ]; then
  PATH="$toolchain:$PATH"; export PATH
fi
if [ -n "$cross" ]; then
  command -v "${cross}gcc" >/dev/null 2>&1 || die 5 "the compiler ${cross}gcc was not found. Pass --toolchain DIR (the Buildroot host/bin folder of the Mac image) or --cross-compile PREFIX."
  say "compiler: ${cross}gcc ($("${cross}gcc" --version | head -1))"
else
  say "compiler: host gcc ($(gcc --version | head -1)); the image was built with: $cc_text"
  say "          if the build fails for a compiler mismatch, build the image first or pass --toolchain."
fi
strip_cmd="${cross}strip"; command -v "$strip_cmd" >/dev/null 2>&1 || strip_cmd=strip
command -v "$strip_cmd" >/dev/null 2>&1 || die 4 "strip was not found (binutils)"

# ---- scratch and output -----------------------------------------------------------------------------------------
if [ -z "$work" ]; then work=$(mktemp -d "${TMPDIR:-/tmp}/wl-pack.XXXXXX") || die 4 "cannot make a scratch folder"; else mkdir -p "$work" || die 4 "cannot make $work"; fi
work=$(CDPATH= cd -- "$work" && pwd)
cleanup() { if [ "$keep" -eq 0 ]; then rm -rf "$work"; else say "scratch kept: $work (wl.ko is in $work/src/amd64; never commit it)"; fi; }
trap cleanup EXIT
mkdir -p "$out" || die 4 "cannot make $out"
out=$(CDPATH= cd -- "$out" && pwd)
cache="${XDG_CACHE_HOME:-$HOME/.cache}/iyakki/wl-pack"; mkdir -p "$cache" || die 4 "cannot make the download cache $cache"   # outside the output folder: that goes on a USB stick

# ---- Broadcom's licence ------------------------------------------------------------------------------------------
fetch_to() { # url file
  if [ "$fetch" = curl ]; then curl -fL --proto '=https' --tlsv1.2 --retry 2 --connect-timeout 30 --max-time 600 -o "$2" "$1"
  else wget -q --https-only -O "$2" "$1"; fi
}
sha_of() { sha256sum "$1" | awk '{print $1}'; }

step "Broadcom's driver source"
if [ -n "$source_file" ]; then
  [ -f "$source_file" ] || die 6 "--source $source_file is not a file"
  srcfile=$source_file
else
  srcfile="$cache/$SRC_NAME"
  if [ -f "$srcfile" ] && [ "$(sha_of "$srcfile")" = "$SRC_SHA256" ]; then
    say "using the copy already downloaded: $srcfile"
  else
    say "downloading from Broadcom: $SRC_URL"
    fetch_to "$SRC_URL" "$srcfile.part" || { rm -f "$srcfile.part"; die 6 "the download from Broadcom failed. Download the file yourself from the address above and run again with --source FILE."; }
    mv "$srcfile.part" "$srcfile"
  fi
fi
got=$(sha_of "$srcfile")
if [ "$got" != "$SRC_SHA256" ]; then
  die 6 "the Broadcom tarball does not match the checksum this script pins.
    expected $SRC_SHA256
    got      $got
  Either Broadcom changed the file (then check it and update SRC_SHA256 in this script on purpose), or the download is damaged."
fi
say "sha256 ok: $got"

mkdir -p "$work/src/amd64" "$work/lic"
tar -xzf "$srcfile" -C "$work/src/amd64" || die 6 "cannot unpack $srcfile"
[ -f "$work/src/amd64/lib/LICENSE.txt" ] || die 6 "the tarball has no lib/LICENSE.txt: this is not the file the script expects"
[ -f "$work/src/amd64/lib/wlc_hybrid.o_shipped" ] || die 6 "the tarball has no lib/wlc_hybrid.o_shipped: this is not the 64-bit hybrid driver"

step "Broadcom's licence (from lib/LICENSE.txt in the tarball)"
if command -v iconv >/dev/null 2>&1; then iconv -f WINDOWS-1252 -t UTF-8 "$work/src/amd64/lib/LICENSE.txt" > "$work/lic/LICENSE.txt" 2>/dev/null || cp "$work/src/amd64/lib/LICENSE.txt" "$work/lic/LICENSE.txt"; else cp "$work/src/amd64/lib/LICENSE.txt" "$work/lic/LICENSE.txt"; fi
cat "$work/lic/LICENSE.txt"
say ""
say "----------------------------------------------------------------------------------------------"
say "That is Broadcom's licence for the driver. It is NOT free software: IYAKKI does not ship it."
say "You are building it for your own machine. The pack this script makes contains Broadcom's software:"
say "do not publish it; if you ever give it to anyone, give them this licence with it (section 2.3)."
say "Loading the module marks the Linux kernel as tainted."
say "----------------------------------------------------------------------------------------------"
if [ "$accept" -ne 1 ]; then
  die 3 "you have not accepted Broadcom's licence. Read it above, and if you accept it run again with:
      sh $0 --accept-broadcom-licence"
fi

# ---- patches ---------------------------------------------------------------------------------------------------
step "Kernel-compatibility patches"
pdir="$work/patches"; mkdir -p "$pdir"
if [ -n "$patches_dir" ]; then
  [ -d "$patches_dir" ] || die 6 "--patches $patches_dir is not a folder"
  cp -R "$patches_dir"/. "$pdir"/ || die 6 "cannot copy $patches_dir"
  say "using your patches from $patches_dir"
else
  pfile="$cache/$PATCH_NAME"
  if [ -f "$pfile" ] && [ "$(sha_of "$pfile")" = "$PATCH_SHA256" ]; then
    say "using the copy already downloaded: $pfile"
  else
    say "downloading Debian's patches: $PATCH_URL"
    fetch_to "$PATCH_URL" "$pfile.part" || { rm -f "$pfile.part"; die 6 "the download of the patches failed. Get $PATCH_NAME from Debian (or the AUR/RPM Fusion broadcom-wl patch set) and use --patches DIR."; }
    mv "$pfile.part" "$pfile"
  fi
  got=$(sha_of "$pfile")
  [ "$got" = "$PATCH_SHA256" ] || die 6 "the patch archive does not match the pinned checksum.
    expected $PATCH_SHA256
    got      $got"
  say "sha256 ok: $got"
  mkdir -p "$work/deb"
  tar -xJf "$pfile" -C "$work/deb" debian/patches || die 6 "cannot unpack $pfile"
  cp -R "$work/deb/debian/patches"/. "$pdir"/
fi
if [ -f "$pdir/series" ]; then plist=$(grep -v '^[[:space:]]*#' "$pdir/series" | awk 'NF{print $1}'); else plist=$(cd "$pdir" && ls *.patch 2>/dev/null | sort); fi
[ -n "$plist" ] || die 7 "no patches found in $pdir"

work_src="$work/src"
n=0
for p in $plist; do
  [ -f "$pdir/$p" ] || die 7 "patch $p is listed but missing"
  if grep -q '^\(diff --git a/\|--- a/\)amd64/' "$pdir/$p" 2>/dev/null; then
    # Debian layout: the package holds amd64/ and i386/; keep only the amd64 parts, apply from the folder above.
    # (a file section starts at `diff --git` or, in quilt-style patches without it, at `--- a/...`)
    awk '/^diff --git /{keep=($3 ~ /^a\/amd64\//); hdr=1} /^--- (a\/|\/dev\/null)/{ if (!hdr) keep=($2 ~ /^a\/amd64\//); hdr=0 } keep' "$pdir/$p" > "$work/cur.patch"
    [ -s "$work/cur.patch" ] || continue
    ( cd "$work_src" && patch -p1 -s --batch --no-backup-if-mismatch < "$work/cur.patch" ) > "$work/patch.log" 2>&1
  else
    ( cd "$work_src/amd64" && patch -p1 -s --batch --no-backup-if-mismatch < "$pdir/$p" ) > "$work/patch.log" 2>&1
  fi
  if [ $? -ne 0 ]; then
    cat "$work/patch.log" >&2
    die 7 "the patch '$p' does not apply to Broadcom's source.
  The pinned set is for Broadcom 6.30.223.271 and kernels up to 6.12. If you changed --source or --patches, the two do not fit.
  Debian's package broadcom-sta carries newer patches for newer kernels; use them with --patches DIR."
  fi
  n=$((n + 1))
done
say "applied $n patch(es)"
srcdir="$work_src/amd64"

# Debian's first patch makes the Makefile pick wlc_hybrid.o_<arch>; its packaging copies the shipped file to that name
if grep -q 'wlc_hybrid.o_amd64' "$srcdir/Makefile"; then cp -p "$srcdir/lib/wlc_hybrid.o_shipped" "$srcdir/lib/wlc_hybrid.o_amd64"; fi

# ---- a private copy of the kernel tree --------------------------------------------------------------------------
step "Making a private copy of the kernel tree (the image's own tree is never written to)"
kt="$work/kernel"
mkdir -p "$kt"
if [ -n "$bad" ]; then
  say "preparing a full copy (modules_prepare needs the sources): this takes a few minutes"
  ( cd "$tree" && tar --exclude='*.o' --exclude='*.a' --exclude='*.ko' --exclude='*.cmd' --exclude='./.git' --exclude='./vmlinux*' -cf - . ) | ( cd "$kt" && tar -xf - ) || die 5 "cannot copy the kernel tree"
  say "running modules_prepare in the copy"
  if ! make -C "$kt" ARCH=x86_64 ${cross:+CROSS_COMPILE=$cross} -j"$jobs" modules_prepare > "$work/prepare.log" 2>&1; then
    tail -40 "$work/prepare.log" >&2
    die 5 "make modules_prepare failed in the copy (last 40 lines above; full log $work/prepare.log).
  Usually the compiler differs from the one that configured the tree: pass --toolchain with the image's cross compiler."
  fi
  [ -f "$tree/Module.symvers" ] && cp -p "$tree/Module.symvers" "$kt/Module.symvers"
  [ -f "$kt/Module.symvers" ] || die 5 "the tree has no Module.symvers: the image kernel was never built, so modules cannot be linked against it. Build the image (dist/buildroot/build.sh mac) first."
else
  # the headers, build scripts and configuration are enough for external modules; the compiled kernel objects are not
  ( cd "$tree" && tar --exclude='*.o' --exclude='*.a' --exclude='*.ko' --exclude='*.cmd' --exclude='./.git' \
      --exclude='./Documentation' --exclude='./drivers' --exclude='./fs' --exclude='./net' --exclude='./sound' --exclude='./samples' \
      --exclude='./virt' --exclude='./vmlinux*' --exclude='./System.map' --exclude='./mm' --exclude='./security' --exclude='./crypto' \
      --exclude='./block' --exclude='./io_uring' --exclude='./rust' --exclude='./tools/testing' --exclude='./tools/perf' \
      --exclude='./tools/lib' --exclude='./tools/bpf' --exclude='./tools/power' --exclude='./tools/net' --exclude='./tools/usb' \
      -cf - . ) | ( cd "$kt" && tar -xf - ) || die 5 "cannot copy the kernel tree"
  mkdir -p "$kt/tools/objtool"
  [ -x "$tree/tools/objtool/objtool" ] && cp -p "$tree/tools/objtool/objtool" "$kt/tools/objtool/objtool"
fi
# a stock module of the image kernel, to compare the full build flags (vermagic) with
stock=$(find "$tree" -name '*.ko' -type f 2>/dev/null | head -1)
[ -z "$stock" ] && stock=$(find "$tree/../../target/lib/modules" -name '*.ko' -type f 2>/dev/null | head -1)
[ -n "$stock" ] && say "reference module: $stock" || say "note: no stock module found to compare build flags with; only the kernel release is checked"

# ---- compile ----------------------------------------------------------------------------------------------------------
step "Compiling wl.ko for kernel $rel"
# -Wno-error: the kernel config turns warnings into errors, and Broadcom's old code has harmless ones (deprecations, fallthrough)
if ! KBUILD_NOPEDANTIC=1 make -C "$kt" ARCH=x86_64 ${cross:+CROSS_COMPILE=$cross} KCFLAGS=-Wno-error "M=$srcdir" -j"$jobs" modules > "$work/build.log" 2>&1; then
  echo "---- last 40 lines of the build log ----" >&2
  tail -40 "$work/build.log" >&2
  cp "$work/build.log" "$out/last-build-failure.log" 2>/dev/null
  die 8 "compiling wl failed (log: $out/last-build-failure.log).
  Typical causes: kernel headers missing (see the kernel-tree messages above), a compiler that differs from the one that built the
  kernel, or a kernel newer than the patches support (then use newer patches with --patches DIR)."
fi
ko="$srcdir/wl.ko"
[ -f "$ko" ] || die 8 "the build finished but there is no wl.ko in $srcdir (last lines of the log):
$(tail -10 "$work/build.log")"
grep -c 'warning:' "$work/build.log" | sed 's/^/compiler warnings (harmless, not errors): /'

# ---- check the module against the kernel -----------------------------------------------------------------------
step "Checking the module against the image kernel"
"$strip_cmd" --strip-debug "$ko" || die 10 "strip failed on $ko"
if [ -n "$stock" ]; then ref="--reference $stock"; else ref=""; fi
# shellcheck disable=SC2086
insp=$("$iyakki" pkg inspect-module "$ko" --kernel "$rel" $ref 2>&1) || { printf '%s\n' "$insp" >&2; die 9 "the module does not match the image kernel $rel (see above). Nothing was packed."; }
printf '%s\n' "$insp"
vm=$(printf '%s\n' "$insp" | sed -n 's/^vermagic: //p')
case $vm in "$rel "*) : ;; *) die 9 "vermagic '$vm' does not start with the kernel release '$rel'" ;; esac
printf '%s\n' "$insp" | grep -q '^depends:' || die 9 "could not read the module's dependencies"
deps=$(printf '%s\n' "$insp" | sed -n 's/^depends: //p')
say "the module depends on: ${deps:-nothing} (the device loads these first; the image kernel has them as modules)"
kobytes=$(wc -c < "$ko" | tr -d ' ')
[ "$kobytes" -lt 8000000 ] || die 10 "wl.ko is $kobytes bytes; a pack takes at most 8 MiB per module"

# ---- the pack -----------------------------------------------------------------------------------------------------------
step "Making the driver pack"
[ -n "$version" ] || version="1.$(date +%y%m%d).$(date +%H%M)"
kosha=$(sha_of "$ko")
built=$(date +%Y-%m-%d)
why="Broadcom BCM4360 Wi-Fi for the Mac mini 2014 and similar Macs. This is Broadcom's proprietary wl driver, a non-free driver that taints the kernel when it loads. You built it on $built for kernel $rel from Broadcom's source with community patches. It replaces the open b43 and bcma drivers for this card. WPA2 networks work, WPA3-only networks do not. It is checked again at every boot and will not load into any other kernel."
payload="$work/payload.json"
{
  printf '{"v":1,"matches":['
  first=1
  for id in 43a0 4360 43a1 43a2 43b1; do
    [ $first -eq 1 ] || printf ','
    first=0
    printf '{"dmi_vendor":"Apple*","pci":"14e4:%s"}' "$id"
  done
  printf '],"modules":[{"name":"wl","kernel":"%s","sha256":"%s","data":"' "$rel" "$kosha"
  base64 < "$ko" | tr -d '\n'
  printf '"}],'
  # `install` lines are not allowed in a pack: only options, blacklist and softdep. These keep the open drivers away
  # from the chip (bcma binds BCM4360 PCI ids, b43 and brcmsmac are its users) and are unloaded now if already loaded.
  printf '"modprobe":["blacklist b43","blacklist b43legacy","blacklist brcmsmac","blacklist bcma","blacklist ssb"],'
  printf '"unload":["b43","b43legacy","brcmsmac","bcma","ssb"],'
  printf '"load":["wl"],'
  printf '"why":"%s"}' "$why"
} > "$payload"
"$iyakki" pkg driver-check "$payload" --kernel "$rel" ${stock:+--reference "$stock"} || die 10 "the pack payload was refused by the device's own checks (see above)"

# publisher key
if [ -z "$keyfile" ]; then keyfile="${XDG_CONFIG_HOME:-$HOME/.config}/iyakki/publisher/wl-owner.key"; fi
keybase=${keyfile%.key}
if [ ! -f "$keyfile" ]; then
  say "no publisher key yet: making one at $keyfile (private: keep it off the USB stick and off shared machines)"
  mkdir -p "$(dirname "$keyfile")" || die 10 "cannot make the key folder"
  chmod 700 "$(dirname "$keyfile")" 2>/dev/null
  "$iyakki" pkg keygen "$keybase" || die 10 "key generation failed"
  [ -f "$keybase.key" ] || die 10 "keygen did not write $keybase.key"
  keyfile="$keybase.key"
fi
pubkey=""
if [ -f "$keybase.pub" ]; then pubkey=$(tr -d ' \n\r' < "$keybase.pub"); fi
[ ${#pubkey} -eq 64 ] || die 10 "the public key file $keybase.pub is missing or not 64 hex characters (delete both key files to make a new pair)"

me=${USER:-owner}
cat > "$work/meta.json" <<EOF
{"title":"Broadcom BCM4360 Wi-Fi (self-built wl)","summary":"Non-free wl driver, built by you for kernel $rel. Taints the kernel.","description":"Broadcom's proprietary wl Wi-Fi driver for the BCM4360 card of the Mac mini 2014, built on $built by $me for kernel $rel. NON-FREE: Broadcom's licence, not GPL; IYAKKI does not ship it. Loading it marks the kernel as tainted. WPA2 works, WPA3-only networks do not. For your own machine only; do not share this file.","publisher":{"id":"$PUB_ID","name":"$me (self-built)","url":""},"categories":["drivers"],"tags":["mac","wifi"],"target":"system","ext_kind":"driver","permissions":["write:/lib/firmware","write:/etc/modprobe.d","run:modprobe","run:insmod"]}
EOF
pack="$out/$PACK_NAME-$version.iyp"
rm -f "$out"/$PACK_NAME-*.iyp
"$iyakki" pkg build "$payload" --kind extension --name "$PACK_NAME" --version "$version" --license LicenseRef-Broadcom-STA --meta "$work/meta.json" --key "$keyfile" -o "$pack" || die 10 "building the signed package failed"
printf '%s\n' "$pubkey" > "$out/wl-publisher.pub"
cp "$work/lic/LICENSE.txt" "$out/BROADCOM-LICENCE.txt"
printf '%s  %s\n' "$(sha_of "$pack")" "$(basename "$pack")" > "$out/SHA256SUMS"

step "Verifying the pack the way the device will"
printf '%s owner\n' "$pubkey" > "$work/trusted.txt"
ver=$("$iyakki" pkg verify "$pack" --trusted "$work/trusted.txt" 2>&1) || { printf '%s\n' "$ver" >&2; die 10 "verification failed"; }
printf '%s\n' "$ver"
printf '%s\n' "$ver" | grep -q 'trust: Trusted' || die 10 "the pack would not show as Trusted even with your key trusted (see above)"
# (`pkg validate` is the check for packages published to a store and rightly refuses a non-permissive licence; this pack is
#  never published, and the device's install review does not apply that rule to a local pack.)

cat > "$out/INSTALL.txt" <<EOF
Broadcom BCM4360 Wi-Fi pack for IYAKKI, built $built for kernel $rel
=====================================================================
This folder holds a driver pack that contains Broadcom's proprietary, non-free Wi-Fi driver ("wl").
It is for YOUR OWN Mac. Do not publish it. If you ever give it to someone, give them BROADCOM-LICENCE.txt with it.
Loading it marks the kernel as tainted. It only loads into kernel $rel: after an image update with another kernel,
run build-wl-pack.sh again.

1. Copy this whole folder to a USB stick.
2. First time only, on the Mac: STORE, Settings, "Trust a publisher key". Paste this key and give it a name such as "My Wi-Fi pack":
       $pubkey
3. Plug the stick into the Mac. STORE, Drivers, "Broadcom BCM4360 Wi-Fi", press "From USB...", give the folder (for example /mnt/usb/$(basename "$out")).
4. Read the review (kernel module, NON-FREE, tainted kernel) and accept. The Wi-Fi networks then show in Settings, Network.
5. If it does not work: Drivers, Why, and the Terminal tile: dmesg | grep -i wl ; ip link ; rfkill list
EOF

step "Done"
say "pack:        $pack  ($(wc -c < "$pack" | tr -d ' ') bytes, signed, kernel $rel)"
say "folder for the USB stick: $out"
say ""
say "Your public key (add it on the Mac: STORE -> Settings -> Trust a publisher key):"
say ""
say "    $pubkey"
say ""
say "Steps:"
say "  1. Copy the folder $out to a USB stick (it holds no private key; your key stays in $keyfile)."
say "  2. On the Mac, once: STORE -> Settings -> Trust a publisher key: paste the line above, name it, confirm."
say "  3. Plug the stick in. STORE -> Drivers -> Broadcom BCM4360 Wi-Fi -> From USB... -> give the folder (e.g. /mnt/usb/$(basename "$out"))."
say "  4. Read the review (non-free, kernel module, taints the kernel), accept. Wi-Fi shows up in Settings -> Network."
say ""
say "Not verified here: loading the module (that needs the Mac). See $out/INSTALL.txt."
exit 0
