# The browser front end's protocol (iyakki-remote)

`iyakki-remote` shows a person's own shell in a web browser. The page (`crates/iyakki-remote/src/page.rs`) is served at `/`; after
sign-in it opens a WebSocket at `/ws?csrf=TOKEN` (same origin, session cookie, one window per session). Everything below is JSON
in WebSocket **text** frames unless said otherwise. A page message the server does not understand is ignored, never trusted.

## Page to server

| `t` | fields | meaning |
|---|---|---|
| `size` | `w`, `h` | window size (clamped to 640x400 .. 3840x2160) |
| `move` `down` `right` `up` | `x`, `y` | mouse, in canvas pixels |
| `wheel` | `d` | scroll steps (-20..20) |
| `key` | `key`, `ctrl`, `shift` | a key press |
| `shift` | `on` | shift held at the next click |
| `paste` | `s` | text from the clipboard (at most 200,000 characters) |
| `full` | | send the whole picture again |
| `sp_voices` | `ok`, `voices: [{n, l, d}]` | the page's speech synthesis: `ok` false when it has none; each voice has name `n`, language `l`, `d` true when it is on the device. Sent when the socket opens, when asked (`speak_voices`) and on the browser's `voiceschanged`. At most 400 voices, names cleaned and cut. |
| `sp_start` | `id` | the browser started saying utterance `id` (`onstart`) |
| `sp_end` | `id` | it finished (`onend`) |
| `sp_error` | `id`, `e` | it failed (`onerror`, `e` is the error name such as `not-allowed`, `synthesis-failed`; `interrupted` and `canceled` are not reported because the server asked for them) |
| `sp_boundary` | `id`, `c` | a word was reached: character position (`onboundary`) |

## Server to page

Binary frames are pictures: four little-endian u16 (x, y, width, height) then a PNG. Text frames:

| `t` | fields | meaning |
|---|---|---|
| `cursor` | `c` | CSS cursor name |
| `copy` | `s` | text for the clipboard |
| `readclip` | | read the clipboard and send it back as `paste` |
| `bye` | | signed out |
| `speak` | `id`, `text`, `voice`, `lang`, `rate` | **browser voice**: say `text` as utterance `id`. `voice` is a voice name from `sp_voices` or null, `lang` a BCP 47 tag such as `ta-IN` (the browser picks a voice for it when `voice` is null), `rate` 0.5..2 (1 is normal). The page cancels whatever is being said first (a new piece replaces the old), waits 40 ms and calls `speechSynthesis.speak`. Text is at most 1,500 characters. |
| `speak_cancel` | `id` or null | cancel utterance `id` if it is the one being said, or whatever is being said when `id` is null. The id matters: a late cancel of an old piece never stops the next one. |
| `speak_pause`, `speak_resume` | | `speechSynthesis.pause()` and `resume()` (the reader does not use them: it cancels and says the piece again) |
| `speak_voices` | | send `sp_voices` again |

The same speech messages are used by the **voice helper** (`docs/VOICE.md`): a small page the desktop window opens in the person's browser, served on `127.0.0.1` by `iyakki-agent::voicehelper` with its own handshake (one-time link token, session secret, `Host` and `Origin` checks) and the same page code (`speech.js`); it adds one message, `{"t":"hb"}` every five seconds from the page, so the server can tell a silent tab from a closed one.

Connecting asks the page for its voices (`speak_voices`); until it answers, the browser voice is "checking". When the socket
closes, the server treats the browser voice as gone and fails what was being said. Speech messages travel on a channel of their
own on the server so a slow page never loses one (pictures may be skipped, speech may not).

Only the text of the story pieces being read is ever sent in `speak`.

Utterance timing: the server reports a piece as failed if `sp_start` has not arrived within 8 seconds (browsers that need a click
inside the page first), and with the page's error name when `sp_error` arrives.

## Where dictation would fit (not built)

The same page could use `SpeechRecognition` (`webkitSpeechRecognition` in Chrome) for dictation into the shell's text fields:
a server message `listen` (`lang`) starting recognition, page messages `sp_text` (`final`, `text`) with the transcript and `sp_listen_end`,
and a `sp_voices`-style capability flag (`recognition: true`). Chrome sends the audio to Google's servers for it, so it would need an
explicit, visible switch and a note in Settings → Voice. Nothing of this exists yet.

## Verified

`crates/iyakki-remote/tests/server.rs` (speech messages reach a page as text frames; the page's events arrive as input; the page
has the speech code) and `tests/chrome_speech.rs` (ignored: the page's real speech code in a real headless Chrome through the
DevTools protocol).
