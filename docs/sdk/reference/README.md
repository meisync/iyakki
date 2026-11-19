# SDK reference (generated)

These pages are written by `python3 dist/sdk/gen-widget-docs.py` from the public API and the doc comments of `crates/iyakki-ui-sdk/src`. Do not edit them: change the comment in the code and run the script. The test `crates/iyakki-ui-sdk/tests/docs_fresh.rs` fails when they are stale.

| Page | What is in it |
|---|---|
| [The app, its context and the host](app-and-host.md) | What an app implements (`UiApp`), what it is handed on every call (`UiCx`), the driver (`Host`), input events and the storage port. |
| [Drawing: Canvas, geometry and the display list](drawing.md) | The one drawing trait, the shapes it uses, the recording canvas and the wire format. |
| [Widgets, size classes, tokens and fonts](widgets-and-style.md) | The small widgets, the size classes and metrics, the design tokens, the font roles and the style-literal lint. |
| [Desktop parts: dock, wallpaper, screens, panes](desktop.md) | What the shell gives an app about its place: arrangement, dock port, wallpaper port, status. |
| [ChatView: the assistant in your layout](chat.md) | The assistant as a component, its port, tools, context and the scripted test port. |
| [BrowserView: a web page in your layout](browser.md) | The embedded browser component and its port. |
| [FilesView: the person's files](files.md) | The file browser component and its port. |
| [TerminalView: the command line](terminal.md) | The terminal component and its port. |
| [EditorView: a text editor](editor.md) | The editor component (the engine is `iyakki-edit`, re-used through the SDK). |
| [AudioView, MediaView and the media port](media.md) | The sound player component and the media port (video is `MediaView`, planned). |
| [Barcodes: Code 128, EAN-13, UPC-A](barcode.md) | In-house encoders (no table to recall) and a view that draws them as dark bars on light paper. |
| [CameraView: a camera in your layout](camera.md) | The camera component and its port. |
| [Events: hear what happens, online and offline](events.md) | The events port, the router with its handlers and actions, and the in-memory port. |
