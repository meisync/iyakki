# __TITLE__

A tile app for the IYAKKI launcher: plain HTML, CSS and JavaScript that runs inside a tile on IYAKKI's own browser engine.

## Files

- `index.html`, `app.js`, `style.css`: the app. Use real `<button>`, `<input>`, `<select>` and `<a>` elements for what people click or type into.
- `app.json`: name, id, version, icon (an emoji or a small .png/.svg), group in the launcher and the capabilities the app asks for.
- `iyakki-app.json`: the store information (licence, summary, categories) used when you pack and publish.
- `tests/*.js`: headless tests (`T.test`, `T.click`, `T.type`, `T.text`, `T.eq`, `T.ok`). Run them with the assistant's `tile_test` or the IDE's Run button.

## The `iyakki` bridge

`iyakki.storage.get/set/remove/list` (capability `storage`, 256 KB per person), `iyakki.ui.toast/title`, `iyakki.host.openFile/saveFile`
(capability `files`), `iyakki.clipboard.read/write`, `iyakki.notify.show`, `iyakki.agent.ask` (capability `agent`, each question needs a yes), `iyakki.app.info()`.
There is no network unless `capabilities.network` lists hosts, no vault, and no `eval`.

## Ship it

Validate (`tile_validate`), test (`tile_test`), add it to your launcher (`tile_install_dev` while you work, or pack, sign and install), then publish to your private store.
