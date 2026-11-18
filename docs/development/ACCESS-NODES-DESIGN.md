# Access nodes for WASM tiles and web content: design delta (F-A11Y-06)

Status: written 2026-11-13 (P-C, round 3). Built 2026-11-15 on the recommended route (ABI 2 beside 1): points 1 to 4 (the `Access` operation, its limits, the order, the device host's mapping into the toolkit tree). Not built: points 5 (web UI hidden list), 6 (Browser tile page nodes in the same tree), 8 (store lint) and a guest SDK helper to emit `Access`. Tests: `iyakki-tile-abi::tests`, `iyakki-tile-host::tests`, `iyakki-shell::shell::tile_wasm_tests`.

## What exists today

* Native tiles: every clickable region becomes an access node (name, role, state) in `iyakki-uikit::access` (F-A11Y-01). The tile does
  nothing; the toolkit reads the label it drew.
* The Browser tile: the page's own tree (`iyakki_web::a11y`) is walked with the keyboard and spoken (`apps/browser_a11y.rs`). It is a
  second path to the screen reader, not nodes in the toolkit's tree.
* WASM tiles (tile ABI 1): a tile returns a display list of rectangles, text and `Hit` regions. A `Hit` has an id and a rectangle and
  nothing else. There is no name, no role, no state. The host can only guess from text drawn near the region.
* The web UI draws a tile on a canvas (`ui/tilerender.js`): a screen reader sees one canvas with an `aria-label` of the tile's title.

## The gap

A person who uses a screen reader, the keyboard walk (F-A11Y-04) or switch access cannot use a WASM tile, including the Runs tile
(F-WEB-55) and any third-party tile, because the host has no names to say and no order to walk.

## Proposal

1. **One new display-list operation, `Access`** (tag 10): `{ id: u32 (the Hit id it describes, or 0 for a region without one), role: u8,
   rect: Rect, name: string (at most 120 bytes), value: string (at most 120 bytes), flags: u8 }`. Roles are the toolkit's: button, field,
   list item, toggle, tab, heading, slider, link, text. Flags: selected, checked, disabled, expanded, secret. A `secret` field carries
   no value (only its length may be said), as in `access::Node::field`.
2. **Limits** next to the others in `limits`: at most 256 `Access` operations per list; names and values cut at 120 bytes on a character
   boundary; a list that breaks a limit is refused like any other bad list.
3. **Order**: the order of the operations in the list is the reading order and the keyboard walk order. A tile that wants another order
   draws the `Access` operations in that order (they draw nothing).
4. **Device host** (`iyakki-tile-host` and the shell's tile frame): each `Access` becomes an `access::Node` for the frame, so the existing
   screen reader mode, keyboard walk and focus ring work unchanged. A `Hit` with no `Access` stays what it is today and is listed by
   `access::unnamed` (the same shrinking list that native tiles have in `shell::access_tests`).
5. **Browser host** (`ui/tiles.js`): a visually hidden list next to the canvas with one real element per `Access` (`button`, `input`,
   `li`, `a`, `h2` ...), in list order, `aria-label` from the name, `aria-checked`, `aria-selected`, `aria-disabled`, `aria-expanded`
   from the flags. Activating an element sends the same pointer events the canvas would (press and release at the centre of the rect), so
   the tile cannot tell a click from an activation. The canvas stays `aria-hidden` once a tile gives any `Access`; a tile that gives
   none keeps today's single labelled canvas.
6. **Page elements of the Browser tile**: `iyakki_web::a11y` nodes are mapped to the same `access::Node` for the frame, so there is one
   tree on the device (the keyboard walk and the screen reader read it), while the browse-mode quick keys stay as they are.
7. **Version**: this is an ABI change. The host refuses a tile whose reported version differs from its own, so the options are: (a) bump
   to ABI 2 and let a host load 1 and 2 (a tile of 1 simply gives no `Access`); (b) keep 1 and let a 1-host skip unknown tags (it does
   not today: an unknown tag is a decode error). Recommended: (a), because (b) would also weaken the check that stops a broken list.
8. **Review rule for the store**: a third-party tile that draws a `Hit` with no `Access` is a lint warning in the store's tile review
   (`store/tile.rs` lint) and an error once the lint has run for one release, so the unnamed controls cannot grow.

## Tests this needs (none written)

* encode and decode of `Access` with hostile input (truncated, over the limits, names that are not UTF-8);
* the Runs tile and the counter describe themselves, and a test fails when a `Hit` has no `Access` (the same ratchet as native tiles);
* node: the hidden list is built in list order and an activation sends the pointer events;
* on a device with the screen reader on: the keyboard walk visits each region, in order, and says the same words as for a native tile.
  This last one needs a real screen reader and real hardware and cannot be done in a harness.

## What the owner has to decide

1. ABI 2 with both versions loaded (recommended) or another way.
2. Whether the web UI's hidden list may replace the canvas for assistive technology (it makes the page structure real, at the cost of a
   second layer to keep in step).
3. Whether the store lint becomes an error after one release.

F-A11Y-06 is `partial` (Impl 50) until the unbuilt points are done.
