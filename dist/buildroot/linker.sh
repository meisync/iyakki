# Optional faster linker for device-profile builds (docs/development/COMPILE-SPEED-PLAN.md, Phase 1 item 3). Sourced by repack.sh and quick-push.sh.
#   IYAKKI_LINKER=lld|mold|auto   use that linker for the cargo call (auto: lld, else mold, else none). Unset, empty or "off": the default
#                                 linker, exactly as before. NEVER required: a missing linker only prints a note and the build goes on.
# Never used for a release program (IYAKKI_PROFILE=release): a release binary must come out the same on every machine. Cargo cannot scope
# rustflags to a profile, so this sets the target's RUSTFLAGS environment value for the one cargo call; that value REPLACES the
# [target.*] rustflags of .cargo/config.toml, so the musl stack-size flag is repeated here (keep the two in step). Changing the flags makes
# cargo rebuild every crate once; choose once and keep it.
# Both linkers are permissively licensed (LLVM lld: Apache-2.0 with LLVM exception; mold: MIT) and are build tools, never shipped.
iyakki_linker_env() { # $1 = rust target, $2 = cargo profile; prints "NAME=VALUE" for `env`, or nothing
  _want="${IYAKKI_LINKER:-off}"
  case "$_want" in off|"") return 0 ;; auto|lld|mold) ;; *) echo "linker: IYAKKI_LINKER must be off, auto, lld or mold (got '$_want'): ignored" >&2; return 0 ;; esac
  [ "$2" != release ] || { echo "linker: IYAKKI_LINKER ignored for a release program" >&2; return 0; }
  _pick=""
  case "$_want" in
    lld|auto) command -v ld.lld >/dev/null 2>&1 && _pick=lld ;;
  esac
  if [ -z "$_pick" ]; then case "$_want" in mold|auto) command -v mold >/dev/null 2>&1 && _pick=mold ;; esac; fi
  [ -n "$_pick" ] || { echo "linker: IYAKKI_LINKER=$_want but no ld.lld / mold is installed: using the default linker" >&2; return 0; }
  _flags="-C link-arg=-fuse-ld=$_pick $(iyakki_remap_flags)"
  case "$1" in *-linux-musl) _flags="-C link-arg=-Wl,-z,stack-size=8388608 $_flags" ;; esac
  _var="CARGO_TARGET_$(printf '%s' "$1" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"
  echo "linker: using $_pick for the cargo call" >&2
  printf '%s=%s\n' "$_var" "$_flags"
}

# No builder path in a shipped program (found 2026-11-13 by the image content gate: /home/NAME/.cargo/registry/... and /home/NAME/PROJECT/... in iyakki and
# iyakki-store-server, from panic and file!() strings). trim-paths is not stable in cargo, so the paths are remapped with rustc flags: the home folder first,
# then the cargo home and the repository (rustc uses the LAST matching prefix). Prints the flags, or nothing when a path has a space (it cannot go through
# `env`). Used by iyakki_export_rustflags and iyakki_linker_env.
iyakki_remap_flags() {
  _h="${HOME:-}"; _c="${CARGO_HOME:-$_h/.cargo}"; _r="${ROOT:-}"
  case "$_h$_c$_r" in *" "*) return 0 ;; esac
  [ -n "$_h" ] && printf '%s' "--remap-path-prefix=$_h=/home/builder"
  [ -n "$_c" ] && printf ' %s' "--remap-path-prefix=$_c=/home/builder/.cargo"
  [ -n "$_r" ] && printf ' %s' "--remap-path-prefix=$_r=/build/iyakki"
  return 0
}
# Sets the target's RUSTFLAGS for the cargo calls that follow in this shell (a musl target also keeps the stack-size flag of .cargo/config.toml, which this
# value replaces). Skipped when the person set RUSTFLAGS themselves, and for a target that is not a musl Linux program.
iyakki_export_rustflags() { # $1 = rust target
  [ -z "${RUSTFLAGS:-}" ] || return 0
  _f="$(iyakki_remap_flags)"; [ -n "$_f" ] || return 0
  case "$1" in *-linux-musl) _f="-C link-arg=-Wl,-z,stack-size=8388608 $_f" ;; *) return 0 ;; esac
  _v="CARGO_TARGET_$(printf '%s' "$1" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"
  export "$_v=$_f"
}
