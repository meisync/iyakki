# Your first app in 15 minutes

You need the repository, Rust (stable, with `cargo`) and a browser. Nothing else is installed. (Inside the project, build and test with the project's own wrapper if your machine has one; the commands below are plain `cargo`.)

## 1. Make the app (1 minute)

```sh
cd <the repository>
cargo run -p iyakki-core --bin iyakki -- new-app hello --dir ../my-apps --sdk crates/iyakki-ui-sdk
```

This writes `../my-apps/iyakki-app-hello/` as its own project (it depends on the SDK by path and on nothing else from IYAKKI):

| File | What it is |
|---|---|
| `src/lib.rs` | the app: a counter with More and Reset. `UiApp` has `event` and `view` |
| `src/ports.rs` | what the app may ask the machine for (a storage stub) |
| `app.toml` | the manifest: id, title, SDK version, smallest pane, ports |
| `tests/golden.rs` | the checks every app keeps (below) |
| `examples/preview.rs` | the app in a browser tab |
| `README.md` | these commands |

`--sdk` names the SDK crate. In `Cargo.toml` it becomes a path **relative** to the new crate when the crate is inside the SDK's checkout or in a folder beside it (the usual layout of a remote build host that copies each folder next to the others), and an **absolute** path on your machine otherwise. Up to three folders below the folder that holds the checkout counts as "beside", so a project folder with its own repository next to the checkout also gets a relative path. In the absolute case the project builds only on this machine: on a build host or a colleague's computer, edit that one `iyakki-ui-sdk = { path = ... }` line.

Inside the repository tree, leave out `--dir` and `--sdk`: the crate goes to `crates/iyakki-app-NAME`, and the command prints the three lines you must add (workspace member, `dist/scripts/crate-layers.txt` line `iyakki-app-NAME apps`, a `FEATURES.md` row).

`iyakki new-app --help` lists the options. Names are lower case letters, digits and single dashes (2 to 30 characters); `sample`, `settings`, `store`, `chat`, `home`, `shell`, `core` and `system` are taken.

## 2. Test it (about 1 minute the first time)

```sh
cd ../my-apps/iyakki-app-hello
cargo test
```

Five tests run: the first frame equals its golden files at phone, tablet and desktop size (the first run writes `tests/golden/*.txt`: read and commit them); every control has a name, sits inside the pane and is at least 44 units high on touch, also at text 3x; a tap, a key and the storage port work; no styling numbers in the drawing code; the source names no machine access and the manifest has no other IYAKKI dependency.

### If cargo runs on another machine (a build wrapper)

Some setups run cargo on a build host through a wrapper command that copies a folder there and streams the output back. Two things differ, and both are handled:

- `./run.sh app-check APP --cargo "WRAPPER DIR"` runs the wrapper from the checkout root with `--manifest-path` of your app, so only your app's tests run (never the workspace's); a standalone app (not inside a checkout) runs in its own folder. `DIR` is the folder the wrapper copies (the checkout root, or the app folder when standalone). No shim is needed.
- Files a test writes on the build host stay there. The first `cargo test` writes the golden files and passes (it says where they are); to bring them back run `./run.sh app-check APP --update-golden --cargo "WRAPPER DIR"`: the test prints its frames and the command writes `tests/golden/*.txt` here. Read the diff, then commit.

## 3. See it in a browser tab (1 minute)

```sh
cargo run --example preview
# prints http://127.0.0.1:PORT/ ; open it
```

`iyakki-ui-preview` serves the app's display list to the page and applies the clicks and keys the page sends back. It listens on 127.0.0.1 only. The page also shows the accessibility tree, so you can see the names a screen reader will say. Stop it with Ctrl+C. Edit `src/lib.rs`, stop and run again: the build is incremental.

The preview binary of the repository, `cargo run -p iyakki-ui-preview --bin iyakki-ui-preview -- --port 8080`, serves the SAMPLE app only (`--app` must be `sample`); use your own `examples/preview.rs` for your app.

## 4. Change something (5 minutes)

Open `src/lib.rs`. Two rules to keep while you edit:

- Sizes, gaps and radii are steps of the skin's scales: `m.size(TypeRole::Body)`, `m.gap(Space::Md)`, `m.radius(Radius::Md)`, `m.line(Line::Normal)`. A number of 3.0 or more in drawing code fails the `no_styling_numbers` test and the message names the line.
- Every clickable thing is a widget (`widgets::button`, `row`, `field`) or a `Canvas::hit` with a name: `cargo test` fails on a control without one.

Then `cargo test` again. If the picture changed on purpose, delete `tests/golden` and run `cargo test` once more, read the diff, commit it.

## 5. Put it on a device or in a desktop window

Today there are two ways, and they differ in how far along they are. Check the status line of each before you plan around it.

| Way | How | Status |
|---|---|---|
| A tile in the shell | add the app to `iyakki-sec::policy::App` and the lists that name every app, write a small adapter next to `crates/iyakki-apps/src/apps/sample.rs`, add the `Tile` row in `shell.rs`; build the OS image or push the shell | works (SAMPLE is the example); by hand, not generated |
| A WASM tile | `iyakki new-app NAME --tile`, then `rustup target add wasm32-unknown-unknown` and `cargo build --release --lib --target wasm32-unknown-unknown`; the `.wasm` goes in a folder with an `iyakki-app.json` that has a `tile` section | the generated tile loads and runs in the real tile host in a test (`cargo test -p iyakki-agent --lib newapp -- --ignored`); pushing it to a real device was not run for this guide |

For a device that is paired in developer mode (`docs/DEVELOPER-MODE.md`), the push command is `iyakki dev push FOLDER`; for a tile folder, `iyakki sdk check FOLDER` first loads the tile in the same sandbox the device uses and prints the install card text. See [packaging-and-publishing.md](packaging-and-publishing.md).

## 6. Where to go next

Read [concepts.md](concepts.md) for what `UiCx`, `Host` and the ports are, then pick a [recipe](recipes/README.md). When the app changes anything outside itself, read [permissions-and-privacy.md](permissions-and-privacy.md): every such feature needs a chat tool.

Verified: see the "Verified" section at the end of [troubleshooting.md](troubleshooting.md).
