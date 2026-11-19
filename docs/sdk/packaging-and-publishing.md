# Packaging and publishing

What exists today, what is by hand, and what does not exist. Read this before you promise anyone a way to ship.

| Step | Status | How |
|---|---|---|
| The app as a crate, tested | works | `iyakki new-app`, `cargo test` |
| In the OS image as a tile of the shell | works, by hand | register the app (`iyakki-sec::policy::App`, `cloudmode::app_class`, `memgate::class_of`), an adapter next to `sample.rs`, a `Tile` row and `build` arm in `shell.rs`, the crate in `dist/scripts/crate-layers.txt`, rows in `FEATURES.md`. Ships with the next image. |
| As a WASM tile (tile ABI 1) | built, experimental | `iyakki new-app NAME --tile`; `cargo build --release --lib --target wasm32-unknown-unknown`; a folder with the `.wasm` and `iyakki-app.json` (`"tile": {"abi": 1, "name": "...", "caps": [...]}`); loading is behind Settings, Developer mode, WASM tiles (experimental). No names for a screen reader in a tile yet (ABI 2). |
| Check a tile on your computer | works | `iyakki sdk check FOLDER` (and `test`, `shot`, `emulate`) from `iyakki-sdk-emulator`: loads the tile in the device's sandbox, prints the install card text |
| Push to a paired device | works for tile folders | `iyakki dev pair HOST` once (Settings, Developer mode on the device), then `iyakki dev push FOLDER --watch` |
| Validate, pack, sign | works | `iyakki pkg validate .`; `iyakki pkg keygen me` (keep `me.key` out of the repo); `iyakki pkg pack . --key me.key -o app.iyp`; `iyakki pkg verify app.iyp` |
| Publish to a store | works against a store server you run or the private store | `iyakki pkg publish --server URL --token-env STORE_TOKEN app.iyp`; the official catalog is listed on the portal and people add store paths (docs/STORE.md) |
| A native (Rust, `UiApp`) app as a signed package a person installs | **does not exist** | native SDK apps ship in an image today; only the WASM tile and the web-bundle tile are installable packages |
| A store review that runs the conformance checklist | **does not exist** | use [conformance-checklist.md](conformance-checklist.md) by hand |

The `pkg validate` review states the permissions in plain words, checks the licence against the permissive list (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, CC0-1.0, 0BSD, Unlicense, BSL-1.0), scans for secrets (names the line, never prints the secret) and compares the name with look-alikes.

Licence rule: only permissive licences; build in-house before you add a dependency. Third-party tools are downloaded by the person from the vendor, never shipped inside your package.

More: `docs/STORE.md`, `docs/APP-AUTHORING.md`, `docs/DEVELOPER-MODE.md`, `docs/SDK-EMULATOR.md`, `docs/development/TILE-ABI.md`.
