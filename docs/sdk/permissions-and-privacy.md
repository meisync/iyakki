# Permissions, approvals and privacy

## What the core refuses

The core is the only door to the machine. An app asks through a port; the core answers with data, a sentence of refusal, or a card for the person. In particular it refuses:

- a path outside the person's home folder, a link, a hidden name, a path with `..` (the files facet);
- a file over its size limit (a text read is at most 1 MB and valid UTF-8; it is refused with a sentence, never cut);
- a command the person's policy does not allow (the terminal facet), and any change until the person allows it;
- an address the network guard does not allow (the browser and media facets); a stream address asks the person first;
- storage over 256 KB, a key longer than 128 bytes, a value over 64 KB (`PortError`);
- any call to a namespace the app did not declare in `app.toml` (`ports = [...]`; a WASM tile also lists `tile_caps`, checked before anything reaches the core).

A refusal arrives as a sentence in plain words. Show it to the person; do not hide it and do not retry in a loop.

## Approval cards

A change asks the person. The card says what will happen (the tool, the arguments, the file or address). The person allows or denies; `y` and `n` answer while the card shows and the field has no focus. A session grant may cover repeated low-risk asks; "hard" asks (destructive or secret-adjacent: deleting, restoring, passwords, consent) are asked every time and nothing covers them. Your app's own tools: `ToolSpec::new(name, description, json_schema, ask)`. Set `ask = true` for anything that changes data or the machine. Write the description for the card: the person reads it.

Secrets are never typed by the model: a password goes through a masked form the person types into, not through the chat or a tool argument.

## Audit

The core writes one line for each decision: who decided (`auto`, `user`, `system`), what, and the result. The line never contains a secret. An app can show the person the recent lines through the core's own pages; it cannot edit them.

## Privacy rules for an app

- Keep only what the app needs, in the storage port; the data is the person's and is readable by that app only.
- Do not put file contents, addresses or the person's text in `eprintln!`, logs or an error shown to someone else.
- The preview and tests use fakes: never paste real data into a test.
- No personal names, home paths, IPs or secrets in the repository or in a test fixture.

## Every feature is reachable by chat

Owner rule: whatever the person can do on a screen, the assistant can do through a tool. A read is a free tool; a change is a tool that asks every time and uses the same code as the screen. When you add a switch, a setting or an action to your app, add its tool in the same change (see the recipe "A setting with a chat tool" and `docs/development/CHAT-REACHABILITY.md`). A reviewer rejects an app with a control that has no tool.

## When you must stop and ask

A new core facet, a new kind of approval or new wording on a card, a rule about what is private, a change that makes a contract test fail in another crate: these are not decisions for an app author. Open a row and ask the owner.
