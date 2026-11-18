# Reading aloud: engines, the browser voice and the voice helper

IYAKKI reads stories (TALES) and, when asked, chat replies aloud. Nothing that makes speech is bundled in the image or the
program (the good free engines are GPL, see below): IYAKKI **finds engines you already have** and calls them like it calls
`aplay`. With one installed, reading aloud works with no setup.

## What happens by default

When you have chosen nothing in Settings → Voice, the first of these that works is used:

1. **An engine you configured** (Settings → Voice → Text-to-speech: an OpenAI-compatible endpoint, a command, the browser
   voice, or an engine of this computer picked with *Engine*).
2. **The best engine found on this computer** (see the list below). Found once in the background when the program starts,
   kept, and looked for again with **Re-check engines** or the TALES chip **Turn on reading aloud**.
3. **The browser voice**, when IYAKKI is opened in a web browser that can speak (`iyakki web`, then open its address): the
   browser's own voices, nothing to install.
4. **The voice helper** (desktop window only): press **Open voice helper in your browser** (Settings → Voice) or the chip
   **Turn on reading aloud** in TALES when nothing was found. See below.
5. **Text only.** The story is still written and saved; TALES shows one chip, **Turn on reading aloud**, that tries the steps
   above and otherwise opens Settings → Voice, which says what is missing and how to get it.

Speech endpoints of the providers under Models (OpenAI and compatible servers) are offered with one click on the Voice page
but are never switched on by themselves: a hosted endpoint sends your text to that service and may cost money. Ollama has no
speech endpoint, so it does not appear.

## Engines that are found

| Engine | Linux | macOS | Windows | Speaks itself | Writes a WAV (Save audio) | Licence of the engine | Shipped by IYAKKI |
|---|---|---|---|---|---|---|---|
| piper (neural voices) | yes, with a voice file | yes | yes | no (a sound player plays the WAV) | yes | MIT for the original program, but it uses espeak-ng for phonemes (GPL); the successor `piper1-gpl` is GPL-3.0 | no |
| macOS `say` | | yes | | yes | yes | part of macOS | n/a |
| Windows speech (PowerShell, System.Speech) | | | yes | yes | yes | part of Windows | n/a |
| espeak-ng | yes | yes | yes | yes | yes | GPL-3.0-or-later | **no (GPL)** |
| speech-dispatcher (`spd-say`) | yes | | | yes | **no (play only)** | GPL-2.0-or-later | **no (GPL)** |
| SVOX Pico (`pico2wave`) | yes | | | no | yes | Apache-2.0 sources, but Debian and Ubuntu ship it in *non-free*; not checked further | no |
| festival (`text2wave`) | yes | | | no | yes | X11-style for the program; many voices have restricted licences | no |
| espeak (the old one) | yes | yes | | yes | yes | GPL-3.0 | **no (GPL)** |

Order when several are found (best first): piper, say, Windows speech, espeak-ng, spd-say, pico2wave, festival, espeak. An
engine that is installed but cannot be used is listed with why: **piper** needs a voice (a `.onnx` file with its `.onnx.json`)
in `~/.local/share/piper`, `~/piper`, `~/.local/share/piper-voices`, `/usr/share/piper*`, `/usr/local/share/piper`, `/opt/piper`
or IYAKKI's own data folder under `voices/`; **speech-dispatcher** needs a speech module (for example espeak-ng) so that
`spd-say -L` lists voices.

### Lemonade (Kokoro and whisper)

A Lemonade server (AMD Ryzen AI, see `ADMIN-GUIDE.md`) can read aloud with its **Kokoro** model and listen with its **whisper**
models. On Settings → Models choose the Lemonade provider, click `kokoro-v1` → *Use as reading voice…* and pick a voice (Kokoro's
usual ones, `af_sky`, `af_heart`, `am_echo`, `bf_emma`, …; Lemonade has no call that lists voices, so the list is Kokoro v1's usual
set and was not read from a server); or click `Whisper-Base` → *Use as speech-to-text*. Both become an HTTP speech engine at
Lemonade's OpenAI-style address (`…/api/v1/audio/speech`, asked for WAV; `…/api/v1/audio/transcriptions`). The Voice page also
offers "Use kokoro-v1 on lemonade-local" as a one-click engine. Because the answer is a WAV, the word highlight is fitted to the
real length of the sound, and with a whisper model on a Lemonade server the words are timed from the sound itself (see *Word
timing from the sound* below).

### Voice in one click: Whisper tiny + Kokoro (Store → Setup)

Store → Setup → *Voices* has **Voice: Whisper tiny + Kokoro** (`iyakki quick-install voice-pack`, `voicepack.rs`). It adds listening
and a reading voice together and sets both up. It is **recommended** (the star) only where Settings → Voice has no speech engine yet.

* **Route.** Through this machine's **Lemonade** in k3s (it already runs whisper.cpp and Kokoro, and the engines of Settings → Voice
  already speak to it), not a separate server and not a local program: the item needs Lemonade (the order is k3s, Lemonade, then this;
  what is there is not installed again), pulls `Whisper-Tiny` and `kokoro-v1` into it, and nothing is added to the image. It goes to the
  cluster on this machine; with another cluster active Setup refuses and says how to switch back (the *Deploys to* rule of Setup).
* **Review first.** The button never starts it directly: a review opens (it scrolls: arrows, Page keys, the wheel) with what is
  downloaded (Whisper tiny about 75 MB, Kokoro about 330 MB, both estimates), where it runs (the CPU, `127.0.0.1:13305` only), the memory
  (about 8 GB on the machine for Lemonade and k3s, about 1 GB more for the two models, estimates), the languages (Whisper tiny is
  multilingual and the smallest Whisper, so expect more mistakes outside English; **Kokoro reads English only**, other languages need
  Indic-TTS), what is set up, and the **licence texts** (MIT for Whisper, code and weights, and whisper.cpp; Apache-2.0 for the Kokoro-82M
  weights and Lemonade). The button reads "I have read the licences: install"; Esc starts nothing. *Install all recommended* shows the same
  review.
* **espeak-ng.** Kokoro turns text into sounds with a phonemizer that commonly is **espeak-ng (GPL-3.0-or-later)**. It would run only
  inside the downloaded Lemonade image, as a separate program; IYAKKI never links it and never ships it. The review says *Not shipped with
  IYAKKI; downloaded at your click*. (What the Lemonade image really contains was not inspected here.)
* **What it sets.** After the self-test passes, Settings → Voice gets speech to text = `Whisper-Tiny` and reading voice = `kokoro-v1`
  (voice `af_sky`), both on `lemonade-local`, through the same paths as *Use as speech-to-text* and *Use as reading voice*
  (`Services::use_lemonade_stt`, `use_lemonade_voice`). Unlike the plain Kokoro tile (which keeps a voice you chose) this replaces the
  engines set now, because you asked for it after reading the review; the old ones are written down.
* **Self-test.** Kokoro speaks "This is the IYAKKI voice test." and Whisper tiny transcribes that sound; it passes when at least half of the
  words were heard (a tiny model may miss "IYAKKI"). The log says *Self-test passed* or *Self-test failed* with what was heard. A recording
  is not bundled (nobody could record one that is licensed for this), so the sound is Kokoro's own; the microphone and the speaker are not
  used.
* **Nothing half installed.** Only models that are missing are pulled (a second run downloads nothing twice). When a pull or the self-test
  fails, the models this run pulled are deleted again and the log says so; models that were in Lemonade before are never touched; a model
  that cannot be deleted is named. No marker and no settings change are left.
* **Undo.** The popup of an installed pack has **Remove (undo)** (administrators, audited as `quickinstall.voicepack.remove`): the engines
  that were set before come back (only the ones that are still the pack's: a change you made since is kept) and the models the pack pulled
  are deleted from Lemonade. The record is `<data>/quick-install/voice-pack.json`.
* **Tests** (fakes only): `voicepack::tests` (the review and its texts, the order and the item facts, the step and its idempotence, failed
  pull and failed self-test with rollback, the self-test with a fake speech server, the settings written and remembered, Remove, presence)
  and `store::setup::voice_tests` (recommended only without engines, the review before anything starts, the cluster and role refusals,
  Remove). **Not verified:** a real download, the real model ids on a real Lemonade (`Whisper-Tiny` is taken from Lemonade's model list in
  its documentation), a real Whisper tiny hearing real speech, Lemonade's `delete` call (read from its API description), what Kokoro in the
  Lemonade image uses to phonemize, and any sound from a real speaker or any sound into a real microphone.

**Without choosing anything**: when Settings → Voice has no text-to-speech engine and a Lemonade on this computer has a downloaded
text-to-speech model, it reads English (Kokoro's voices are English) before the engines found on this computer; other languages
keep those engines. A server that does not answer is tried once more, then left alone for a minute while the engine of this
computer reads.

**Servers on other machines.** Several Lemonade servers can be saved under Settings → Models (Detect finds one on this
computer; one on the network or the internet is added by its address, http or https, with its key, which stays in the vault and is
sent only to that server). Reading and listening may use different servers. The Models panel says where a server is (*on this
computer*, *on the network*, *on the internet*). Text (to be read) and sound (from the microphone, or made speech to be timed) go to
a server on another machine only after a yes on a card that names it, once per server; until then it is never used, not even by
itself. The yes is kept in the voice settings (`remote_ok`).

### Voices, languages and speed

- The story's language chooses the voice: Tamil, Hindi, Telugu, Malayalam, Kannada, Bengali, Marathi, Spanish, French,
  German, Portuguese, Arabic, Japanese or English, whenever the engine has a voice for it. When it has none, the default voice
  is used and the Test button says so ("no Telugu voice"). espeak-ng has Tamil, Hindi, Telugu and Malayalam; piper has voices
  you download per language.
- *Voice* on the Voice page (and the voice chip in TALES) picks one voice of the engine, for the language shown.
- *Speed* is mapped to what each engine understands: words per minute for espeak and `say` (175 at normal), -100 to 100 for
  speech-dispatcher, -10 to 10 for Windows speech, a length scale for piper and festival. `pico2wave` has no speed.

### How the engines are run

Without a shell. The text of the story or reply goes to the program's **standard input** (only `pico2wave` can take text only
as its last argument; it comes after `--` as one argument, so it can never be taken for an option). Voice names are checked (no
leading `-`, no control characters; festival's names only letters, digits and underscores). Every process has a time limit and is
ended together with its children when you press stop, pause or leave the page; speech-dispatcher also receives `spd-say -C`
because it goes on talking after its client is gone. The commands are in `crates/iyakki-agent/src/hostengines.rs` (`plan`) and
checked in its tests, including text with quotes, new lines and `$(...)`.

## Save audio

Needs an engine that can write a WAV file (everything above except `spd-say`, and not the browser voice). With a play-only
engine the button is dimmed and says so when pressed; choose espeak-ng or piper in Settings → Voice to save.

## The browser voice helper (desktop window)

The desktop window's web engine (WebKitGTK) is often built without speech synthesis. The helper lets your **web browser**
speak for it:

1. Settings → Voice → **Open voice helper in your browser** (or the TALES chip when nothing else works).
2. IYAKKI starts a listener on `127.0.0.1` on a random port and opens your default browser (`xdg-open`, `open` or Windows'
   URL handler) on a small page. Click the button on that page once (browsers only allow sound after a click).
3. The window shows *Voice helper connected*; pieces are spoken by the browser with its voices. **Disconnect voice helper**
   (or closing the tab) ends it.

Security: only `127.0.0.1`; the link has a one-time token; the WebSocket needs a session secret, the exact `Host` and the page's
own `Origin` (a page of another site cannot connect); one tab at a time; it closes when the tab closes, goes quiet for 45
seconds, is not opened within two minutes, or you disconnect. Only the text of the pieces being read is sent to the tab and
nothing leaves the computer through it. The page and protocol are the web front end's (`crates/iyakki-agent/src/speech.js`,
`docs/REMOTE-PROTOCOL.md`).

## Bare-metal and console images

**Nothing is bundled** (licences above), and the image has `aplay` but no speech program or voice. Options, in order of ease:

- **Browser voice via `iyakki web`**: works on any machine that has a browser. Start the browser front end
  (`iyakki.web=1` on the kernel command line, or `iyakki web`), open its address from another computer or phone, sign in, open
  TALES. No installation on the IYAKKI machine at all.
- **piper**: put the `piper` program somewhere on the PATH (`/usr/local/bin`) and a voice (`.onnx` with `.onnx.json`) in the
  data folder's `voices/` directory. The data folder is the IYAKKI-DATA volume. Mind the GPL parts of piper's phoneme library
  when you redistribute an image.
- **espeak-ng** or **speech-dispatcher** in your own image: add the package to your Buildroot configuration (they are GPL: you
  take on the GPL duties for your image; IYAKKI only runs them as separate programs).
- **An OpenAI-compatible speech server** on the network (a provider under Models, or Settings → Voice → Text-to-speech).

### Installing on a normal computer

- Debian, Ubuntu: `sudo apt install espeak-ng` (Tamil, Hindi, Telugu and Malayalam voices are included), or
  `sudo apt install speech-dispatcher speech-dispatcher-espeak-ng`.
- Fedora: `sudo dnf install espeak-ng`. Arch: `sudo pacman -S espeak-ng`.
- macOS: nothing to install (`say`); more voices in System Settings → Accessibility → Spoken Content.
- Windows: nothing to install (voices in Settings → Time & language → Speech).
- piper: download the program and a voice from the piper project.

## The word being said

Reading aloud lights the sentence and the word in TALES and in chat replies (Settings -> Voice -> Highlight: *Sentence + word*, *Sentence
only*, *Off*; key `h`). How the word is known, best first:

| Voice | Source | Exactness |
| --- | --- | --- |
| Browser voice (web front end, voice helper) | `SpeechSynthesisUtterance` `boundary` events (`charIndex`, UTF-16, converted to words of the piece) | Exact for the voices that fire them. Chrome's network voices do not: the highlight then follows the estimate until an event comes. |
| piper, espeak-ng, HTTP endpoints, commands (they make audio) | The measured length of the WAV shared out over the words by a syllable-weighted model with pauses at punctuation | An estimate; typically within a word or two inside a sentence, never accumulating (each piece starts from its first word). |
| HTTP engines with a speech-to-text server (Lemonade Kokoro + whisper) | Native word times when the engine gives them, else what whisper heard aligned to the text (below) | Measured; falls back to the length estimate. |
| `spd-say`, macOS `say`, Windows speech (speak by themselves, no length) | The same model at the speed (words per minute) | A rougher estimate; resynchronised at every piece. |

### Word timing from the sound

When the engine makes audio, better times than the estimate come from the sound (`crates/iyakki-agent/src/speechtime.rs`):

1. **Native**: the engine says when each word is said. Kokoro-FastAPI's captioned speech does (set the speech engine's address to
   `…/dev/captioned_speech`). Lemonade does not: its `/audio/speech` forwards to Kokoros, whose OpenAI server answers with audio
   only.
2. **Aligned**: the piece's sound (as 16 kHz WAV) goes to a speech-to-text server with `response_format=verbose_json` and word
   granularity. Lemonade's whisper.cpp backend answers with `segments[].words[]` (whisper tokens with `start` and `end`; a token
   that starts with a space starts a word). What it heard is matched to the text that was read with an edit-distance alignment:
   only the times are used, the words lit are always the text's own. A misheard word ("box" for "fox"), a skipped word or an extra
   one ("uh") does not move the others; numbers said as words, punctuation and Indic scripts (by their letters) are handled; words
   that were not matched are placed between the matched ones by their length. When fewer than a third of the words match (another
   language or script, or a server that answers without times, such as Lemonade's FLM backend) the estimate stays.
3. **Estimated**: the model above, also while an alignment is still running.

Each piece (a sentence or so) is aligned on its own beside the reading: it starts with the estimate and the lit word moves to the
measured times the moment they arrive; the next pieces are made and aligned while it plays, and a replay, pause, resume or seek
uses them at once. Sound and times are kept by server, model, voice, speed and text. The server that times the words is the
speech-to-text engine of Settings → Voice when it is an HTTP server, else a Lemonade whisper model (the smallest) on this computer
or on a server you said yes to. Settings → Voice → *Word timing* (`w`) turns it off and shows which server is used; the TALES
player line says quietly where the times come from (`words aligned`, `words estimated`, `words native`, `words from the voice`),
and the debug log says it per piece.

Chat reads a reply sentence by sentence with the same reader (so a click on a sentence starts there, **Pause**/**Resume** and **Stop**
sit beside the "speaking" line, and the browser voice reads replies too). What cannot be judged without ears is how closely the
estimate follows a given voice: switch on a story or a long reply and watch; if the lit word runs ahead of or behind the voice, say
which engine and language, and choose *Sentence only* meanwhile. `crates/iyakki-agent/src/wordtime.rs` holds the model (`UNIT_MS`,
the pauses) if a voice needs it tuned.

## Indic neural voices: removed

Removed 2026-10-27: AI4Bharat states no licence for the Indic-TTS model weights, so the Indic voices item, its Settings page, the voice server (`dist/tts-indic`) and the `indic` module were taken out. Other languages: use espeak-ng, piper or a voice of your own through Lemonade (see below).

### English and other options

For Indian English the same engine has a voice (`en`). If you want a different English voice, Kokoro behind any OpenAI-compatible
speech server remains an option (Settings -> Voice -> Text-to-speech, an `/v1/audio/speech` address); it was not evaluated here.

### If a pure-Rust runtime is wanted later

Export splits cleanly: `fastpitch.onnx` takes `tokens [1,T] int64`, `speaker [1] int64`, `scale f32` and returns `mel [1,F,80]`;
`hifigan.onnx` takes `mel [1,F,80]` and returns `wav [1,1,N]` (256 samples per frame). The only dynamic shape step is the length
regulator (`repeat_interleave`), which a runtime without data-dependent shapes would do outside the graph, by splitting
FastPitch in two. Before adding `tract` (Apache-2.0 / MIT) to the build, measure it against onnxruntime on the Haswell Mac mini; a result
under real time with a margin for the vocoder is what would justify it.

## The VOICE tile and the system sounds

The VOICE tile (Home) is a place to try the voices: paste text and have it read, or speak and copy what was heard. It adds no engine of
its own; it uses what Settings -> Models, Settings -> Voice and Store -> Setup set up. The logic is `voicedesk.rs`; the screens are
`apps/voice.rs` and `apps/voice_ui.rs`.

**Which voices are listed** (`voicedesk::read_voices`): the voice set in Settings -> Voice; for every enabled Lemonade provider each
downloaded text-to-speech model, with Kokoro's usual twelve voices (`af_sky`, `af_heart`, `am_adam`, ...: Lemonade has no call that lists
them, so the table in `lemonade.rs` is used); text-to-speech models of other providers (model kind `tts`); and the engines found on this
computer (espeak-ng, piper, ...). A chosen voice is used for this
reading only: Settings -> Voice is not changed.

**What is asked.** Each sentence is one request, `POST <base>/audio/speech` with `{"model": "kokoro-v1", "voice": "af_heart", "input":
"<the sentence>", "response_format": "wav"}` and `"speed"` only when it is not 1.0. Up to two sentences are made ahead of the one that
plays (the same reader as TALES: `story::Reader`), so a long text starts quickly and a stop is immediate. The text box takes 20,000
characters; longer text is cut with a notice that says how much was left out. Sentences with nothing to say (an address, marks) are
skipped. The speech goes to the chosen output through the play command of Settings -> Voice (Settings -> Sound applies).

**Where the text goes.** The page always says: `Text is sent to <provider> at <address> (this computer | your network | the internet)`,
or that it stays on this computer. A server that is not this computer asks first, on a card that names the provider, the host and
where it is; Yes is stored (`remote_ok` in the settings file, `Services::allow_speech_server`) the same way Settings -> Models stores
it, so one yes covers reading, listening and TALES for that server. No sends nothing and remembers nothing. The audit log records that
a reading happened with which voice and how many characters, never the text.

**Dictation.** `Recorder` (arecord by default) runs only after a click or key and for at most five minutes; the samples are kept in
memory and handed to the speech-to-text of Settings -> Voice (or the first Lemonade whisper model) as a WAV in a multipart request
(`model`, `response_format=json`, `file=audio.wav`), then dropped. Nothing is written to disk and there is no debug switch for it.
Microphones are the capture devices of `/proc/asound/pcm` (USB ones marked from `/proc/asound/cards`); the choice is put in the record
command as `-D plughw:C,D`, unless the command names a device itself. Without any, the page explains that USB microphones work and that
the built-in microphones of some laptops are not supported yet; on a MacBookPro13,x/14,x it names the model (the CS8409 codec has no
driver, F-DRIVERS-27).

**System sounds** (`syssounds.rs`). Four cues (login, logout, notification, job done) are made as PCM in code: two or three sine notes
with a little decaying FM, a soft attack and an exponential decay, scaled so the loudest sample is 60 percent of full scale and then
lowered by the level (-12 dB by default), 44.1 kHz mono, first and last sample zero. They are written as a WAV to the play command
(`audioout::route_args`, the software volume of Settings -> Sound, `voice::run_capture`), exactly as the test chime is. `start` returns
at once with a handle; sign-out starts the logout cue first and waits for the handle for at most 700 ms just before the screen changes.
Nothing plays when the master switch or the cue's own switch is off, when Sound is muted, when `audioout` says there is no output
(`NO OUTPUT`), or in cloud mode without sound. The notification cue follows the bell's unread count going up (the first count after
signing in is not new); the done cue follows a finished job (once for a batch) and the top-bar installer indicator going from running
to done. Settings -> System sounds has the switches, the level and a Preview for each cue.

## What was verified and what was not

Tested with fake engine scripts on a temporary PATH (each engine kind: detection, voice lists, command lines, hostile text,
speed, language and voice choice, cancellation kills the process, Save audio flags, the Test button, the order of defaults), with
a raw WebSocket client for the helper (token, origin, one tab, closing) and for the layout and clicks of the Voice page and the
TALES chip at 1280x720, 1920x1080, 1024x600 and 800x480. On the development computer only speech-dispatcher (with its espeak-ng
module) is installed and was found and driven (`spd-say` accepted the flags used); **no sound was heard** (no audio device in
the test setup). espeak-ng, piper, festival, pico2wave, `say` and the Windows script have not been run for real: their flags come
from their manuals. The helper page was not opened in a real browser here (its speech code is the web front end's, which ran in a
real headless Chrome).

`cargo test -p iyakki-agent --lib real_machine -- --ignored --nocapture` prints what the computer it runs on really has.

**Word timing from the sound and remote Lemonade.** Verified against fake servers on loopback only: a fake Lemonade answering a
canned WAV and a canned whisper.cpp `verbose_json` (misheard, missing and extra words, Tamil), a server that fails, one that never
answers and a slow one, Kokoro-FastAPI's captioned answer, the reader switching from the estimate to aligned times while a piece
plays, and the yes card for a server on another machine. What Lemonade sends was read from its source (lemonade-sdk/lemonade,
lemonade-sdk/Kokoros, ggml-org/whisper.cpp `examples/server`, October 2026). **Not verified:** a real Lemonade, a real whisper
(how well its word times follow Kokoro's voice, and how often it hears Tamil well enough to match), and how the aligned highlight
looks and sounds together.

**The VOICE tile and the system sounds.** Verified against a fake Lemonade on loopback (the real request bodies of `audio/speech` and
`audio/transcriptions`), a fake speaker, a fake recorder and a fake output: the request (model, voice, input, `response_format`, speed),
the sentence lit while reading, pause, resume and stop, 20,000 characters cut into pieces, the consent card for a server on another
machine, a server that does not answer, no voice, no sound output, no microphone, a silent recording, a microphone that disappears, the
five minute limit, that nothing is recorded without a click and nothing is written to disk, the transcript copied to the clipboard, the
cues (length, level, no clipping, zero at both ends, rising and falling pitch), that a slow output never holds sign-out back, the
switches and the level of Settings -> System sounds, and both VOICE areas at 360x740, 800x480, 1280x720 and 1920x1080. **Not
verified:** any of it with a real microphone or speakers (nobody listened to the cues or to Kokoro through this page, and the level of
the cues next to the radio was not compared), a real Lemonade or whisper, the Store → Setup hand-off on a device, and a recording of the
full five minutes.

## Omni collections: Use all parts

Lemonade can serve a *collection* (`recipe: "collection.omni"`), a bundle such as `LMX-Omni-5.5B-Lite` (9.3 GB: chat and vision
`Qwen3.5-4B-MTP-GGUF`, pictures `SD-Turbo`, speech in `Whisper-Tiny`, speech out `kokoro-v1`). In Settings -> Models the collection
is a chip marked "omni collection". Click it to see its parts with their state (installed, or missing with the size) and:

- **Use all parts** (administrators): a card says where it runs (provider, host, this computer or another machine) because chat,
  pictures, what the microphone hears and what is read aloud then go to that provider. On yes it sets the chat model, the TALES
  picture model, speech-to-text and the reading voice to the parts, all four or none: a step that fails puts the earlier ones back and
  names the part. For a server on another machine the yes also allows sending speech there.
- **Install missing parts**: when a part (or the whole collection) is not on the server, Use all parts changes nothing and offers this
  instead. It pulls the collection id (Lemonade fetches the parts) with streamed progress; if that download failed before, it pulls
  the missing parts one by one. Press Use all parts again when it is done.
- **Undo Use all parts**: puts back the previous chat model, both speech engines, the provider default and the picture model.

The chat model is set to the collection's chat part. Whether Lemonade also serves chat through the collection's own id (routing tool
calls to the parts) was not established from this code, so the plain part is used; it works either way.

### Trying LMX-Omni-5.5B-Lite

1. In Settings -> Models pull `LMX-Omni-5.5B-Lite` and `SD-Turbo` (Install missing parts does both), then press **Use all parts**.
2. Chat with a picture (vision): attach a screenshot and ask what is on it.
3. Ask the assistant to draw a picture.
4. Dictate a question (Voice) and have the answer read aloud.
5. Make one TALES story with pictures.
6. The model lab Vision and Picture tabs once they exist.

Record for each: wall-clock time (first call includes loading), quality (good, poor, wrong), and the server's memory use while the
parts are loaded (for example with `free -h` or the Lemonade log) and any part that Lemonade unloads to make room. Press Undo
afterwards if the old setup should come back.

## My IYAKKI (push to talk), Voice -> ASSISTANT (F5)

A voice assistant joined to the chat assistant (F-VOICE-28, [`development/MY-IYAKKI-PLAN.md`](development/MY-IYAKKI-PLAN.md)). Off by default; an administrator turns it on in the tab (or with the chat tool `voice_assistant_set`, which always asks and can only be approved by a tap or key). Press Talk (or Space), speak, pause; the assistant answers aloud in short sentences and listens again for a few seconds for a follow-up. The microphone is open only while the red dot shows. Say stop, cancel, louder, quieter, mute, repeat or that's all at any time. A tool that asks shows a card: ordinary asks can be answered by saying yes or no; deleting, installing, sending, sharing and the rest of the hard-ask list can only be approved by a tap or key (saying no still denies). If the assistant is unsure what it heard it asks "Did you say ...?" first. No sound is kept; the conversation is saved as text in a chat titled "Voice conversation". Settings: `voice.my_iyakki`, `voice.my_iyakki_muted`, `voice.my_iyakki_follow_ms`. The wake phrase is not built yet.
