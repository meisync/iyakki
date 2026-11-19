# F-BJS-01: clicks and typing reach the scripts of a page the reader shows

Status 2026-11-13: design written; first slice built (crate level, see "Slice 1"); the browser does not use it yet (KI-716).

## What happens today

The built-in reader loads a page, runs its scripts once (`iyakki_js::run_page`: scripts, timers played out in virtual time, `load`
handlers), takes the finished tree, builds a `Doc` from it and drops the script engine. After that the page is a picture of text, links
and form controls: a click on a button whose `onclick` or `addEventListener('click')` handler changes the page does nothing, typing into
a field that a script watches does not reach the script, a page that builds its menu on click stays shut. Pages that need this are
opened in the full web engine (F-FULL), which is the right place for video, canvas and heavy applications.

`iyakki_js::LivePage` keeps the engine alive and already has `click`, `input`, `check`, `advance` (timers) and `tree()` (the page as
it is now). Today only the developer tools' inspector uses it (`browser_inspecthost.rs`).

## Goal and limit

For pages the reader shows: a click on a control or a link, typing, and checking a box are delivered to the page's scripts; the page is
drawn again from the changed tree; a script that asks to go to another address after a click is followed like a script navigation at
load. Not a goal: video, canvas drawing, drag and drop, hover effects, scroll-driven scripts, anything that needs real time to pass
between frames. Those stay with the full engine. The page says when it is a scripted page whose interactions are only partly supported.

## Pieces

1. **Keep the engine per tab.** The loader thread that runs `run_page` today runs `LivePage::load` instead (same steps, the engine is
   handed back). One live page per tab, dropped on navigation, on closing the tab and when the tab has been in the background for a
   few minutes (it is then rebuilt by loading again when the person comes back; nothing is lost that was not lost before). The engine
   lives on its own thread (the wasm engine is not meant to be driven from the screen thread); the shell sends it `Interaction`s over a
   channel and gets `Outcome`s back, so a slow handler never freezes the screen. Memory: the engine is counted in the tab's budget
   (F-BROX-12 style caps) and the number of live engines is capped (default 4).
2. **Name the target.** `Control::node` (already in `Doc`) is the node of the tree a form control was made from. Links and
   non-control elements with handlers need the same: `Doc` gets a list of `(block range, node)` for elements that have a listener or an
   `onclick` attribute, so a click on the drawn text can be turned into `Interaction::Click(node)`. Without a listener a click on plain
   text does nothing, as today.
3. **Deliver.** `LivePage::interact(Interaction)` (slice 1): click, input (the `input` and `change` events with the value set before
   them), check. Key events and focus/blur come later. Each interaction gets a time budget in virtual time (50 ms of timers play out
   after it, as for `click` today) and a real-time cap; a handler that loops is cut off like a script at load and the page says so in
   its notes.
4. **Draw again.** `Outcome::tree` is built into a `Doc` with the same style sheets and viewport as the first time (the loader keeps
   the `StyleSheets` it built), images go through the picture cache, the scroll position stays, and the focus stays on the control the
   person was using. A cheap check (`grew_by`, then a comparison of the built `Doc`) avoids drawing again when nothing changed.
5. **Navigate.** `Outcome::navigation` is the address a script asked for during the interaction (taken once). The browser follows it
   with the same checks as any address (host policy, mixed content, the known-bad list).
6. **Forms.** A submit button with a `submit` handler that calls `preventDefault` must not submit; today the reader submits first.
   The live page reports `default_prevented` for click and submit so the reader skips its own action.
7. **Limits and safety.** No new capability: the engine keeps the same modules, the same `allow` closure and the same mixed-content
   rules as at load. Console output goes to the developer panel as today. A page that fetches in a handler uses the same network
   transport (so the filter, the known-bad list and the cache apply).
8. **Tests.** Crate level (slice 1); the browser on a fake site with a page that counts clicks, filters a list while typing and opens
   a menu; a handler that throws, loops or navigates; a tab left in the background and brought back; the engine count cap; the
   screen thread never waits for a slow handler (a fake slow engine).

## Slice 1 (built)

`iyakki_js::LivePage::interact(&Interaction) -> Outcome`: click, input and check by node, the changed tree, the console lines, and
the address a script asked to go to (taken, so it is not reported twice). Tested on a page that counts clicks, reacts to typing and a
check box, and navigates after a click, using the controls' own `node` numbers from a `Doc` built from the tree, and a page whose
handler throws. This is the part that decides whether the idea works with the engine we have: it does.

## Not built (what is left, in order)

1. The loader keeps the engine and the engine thread with its channel (piece 1).
2. Targets for non-control elements (piece 2) and the click path in the browser (pieces 3 and 4).
3. Navigation after a click and `preventDefault` (pieces 5 and 6).
4. Key events, focus, and timers that run while the person looks at the page (a slow tick of real time into the engine).
5. Background tab dropping and the engine cap.

## Open questions for the owner

* Should a scripted page that the reader cannot fully drive (video, canvas) offer "Open in the full browser" right under the address bar
  instead of only the note? (Proposed: yes.)
* Real-time timers for live pages cost battery; proposed default: virtual time only, plus a real tick of 1 per second while the tab
  is visible.
