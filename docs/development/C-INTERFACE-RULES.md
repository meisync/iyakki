# C interface rules (charter step 7, F-CORE-22)

Every `#[no_mangle] extern "C"` function is a promise to code written in another language: the WASM tiles, the page glue, and the
`iyakki-core` shared library when it is built (docs/development/CORE-ACCESS-MODEL.md step 4). The rules keep that promise cheap.

1. A symbol name is never reused for another meaning. A changed or removed symbol is recorded as a `removed` row with a note; the
   replacement gets a new name.
2. Only plain data crosses: integers, floats, pointers with a length, UTF-8 bytes. No Rust types, no panics across the boundary,
   no callbacks without a version.
3. Memory is owned by whoever allocated it, and freed by the matching exported function (`*_alloc` / `*_free`).
4. Every crate that exports C functions answers its interface version with a function ending in `_abi_version` (the tile ABI:
   `iyakki_tile_abi`), or has an `exempt` row that says why it need not.
5. Strings and structures are JSON or a documented byte layout with a version field; unknown fields are ignored.
6. Errors are return codes (0 = ok, negative = refusal); the reason is read by a second call, never thrown.

The gate is `crates/iyakki-core-api/tests/c_abi.rs` against `docs/development/c-abi-symbols.txt`: a new export is appended with
`IYAKKI_C_ABI_WRITE=1`; a removal or changed signature also needs `IYAKKI_C_ABI_NOTE="why and what replaces it"`.
