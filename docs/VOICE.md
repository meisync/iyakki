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

## Indic neural voices (AI4Bharat Indic-TTS)

espeak-ng reads Tamil, Telugu, Malayalam and Hindi, but robotically. **Indic voices** are neural text-to-speech (FastPitch for the
sounds, HiFi-GAN for the waveform) from AI4Bharat's Indic-TTS project: Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi,
Gujarati, Punjabi, Odia, Assamese, Bodo, Manipuri and Indian English, each with a female and a male voice, 22 050 Hz. They are an
engine kind of their own, **Indic neural voice (local)**, chosen in Settings -> Indic voices (or the Engine chooser on the Voice
page). **Default: off.** Nothing is bundled and nothing is downloaded by itself.

### How it is built, and why

```
 story text --> IYAKKI (Rust): normalise, numbers, Latin words, pieces, voice for the language
                  |  one HTTP request per piece: POST /v1/audio/speech  {input, voice: "ta-female", speed}
                  v
            voice server (dist/tts-indic/server.py: numpy + onnxruntime; this computer, a PC, or a k3s service)
                  |  WAV 22 050 Hz mono
                  v
 IYAKKI: silence trimmed, same loudness for every piece, faded edges, pauses by punctuation, one WAV -> player / Save audio
```

* **Why a server and not the Rust program itself.** The two networks are 54 M (FastPitch) and 14 M (HiFi-GAN V1) parameters. On the
  development computer (AMD Ryzen 9 7940HS, onnxruntime) they run at 0.35 of real time with two threads. A Haswell i5 (the Mac mini
  class) is several times slower per core, so the same program would sit around real time or above, and a hand-written Rust
  convolution loop or `tract` would be slower than onnxruntime, not faster. A pure-Rust path was therefore **not built**: it would
  add a dependency, was not shown to be fast enough, and the server path is already complete. The ONNX files are exactly what a
  later Rust runtime would load (see "If a pure-Rust runtime is wanted" below).
* **One frontend, in Rust.** Everything that knows languages is in `crates/iyakki-agent/src/indic/` and is the same wherever the server
  runs: `text.rs` (symbols, scripts, Latin words, pieces), `numbers.rs`, `audio.rs`, `models.rs` (the files on the device). The
  server only does the model's own cleaner (lower-case, `; : -`, drop `< > ( ) [ ] "`) and drops characters the model does not know.
* **The engine setting** is `Host { engine: "indic-tts", program: "<server address>", voice: "female" | "male" | "ta-male" }`, so Save
  audio, the player, the word highlight (the WAV length is real) and the Test button work as for every engine of this computer.
  `program` is an address, not a file; the Voice page shows the server's host so you always see where the text goes.

### Settings -> Indic voices (administrators)

A list of the languages with their state. Keys (also buttons): `d` **download** (an approval card first: the host the files come from,
how big, the licence notice; nothing is fetched before `y`), `x` delete (asks), `u` install **from a folder or USB stick** (for a
computer that is never online; every file is checked first), `b` where the bundles are published, `a` the server address, `e` use
for reading on or off, `g` female or male, `y` test voice, `c` check the server, `k` cancel a download. Files are written as `*.part`,
continued after an interruption (`Range`), checked against a pinned SHA-256 and size, and only then renamed into place; a file that
does not match is deleted and never used. Models live in `<data>/tts/indic/<lang>/{fastpitch.onnx, hifigan.onnx, vocab.json}`
(`<data>` is the IYAKKI-DATA volume or the window's data folder) and never leave the device.

**Where the bundles come from.** AI4Bharat publishes PyTorch checkpoints (1.5 GB per language, with training state), not ONNX files.
`dist/tts-indic/tools/export_onnx.py` makes the ONNX bundle of a language on a PC (260 MB: FastPitch 207 MB, HiFi-GAN 53 MB, a
2 KB `vocab.json`); `tools/pack_bundle.py` prints their checksums. Publish the three files of each language at
`<address>/<lang>/<file>` (a private store, a web server, a k3s service) and type the address with `b`. IYAKKI has the checksums
of the reference build (the one measured below) built in; for your own export use `tools/pack_bundle.py > <data>/tts/indic/catalog.json`,
which the page prefers. **The address is empty until an administrator sets it.** No bundle is hosted by IYAKKI.

### Running the server

On a PC or Mac (Python 3.10 to 3.13 with numpy and onnxruntime; nothing else):

```
python3 -m venv ~/tts-venv && ~/tts-venv/bin/pip install numpy onnxruntime
~/tts-venv/bin/python dist/tts-indic/server.py --models "<data>/tts/indic" --listen 127.0.0.1:8189 --threads 2
```

`GET /healthz`, `GET /v1/voices` (the installed voices), `POST /v1/audio/speech` (`input`, `voice` as `<lang>-<female|male>`,
`speed`). A language without models answers 404 `model_not_installed`; IYAKKI then reads with the next engine and says why in one
line (Settings -> Indic voices, "Last fallback"). The server holds two languages in memory at most, handles one request at a time,
and refuses bodies over 64 KB and input over 1500 characters.

On k3s: Settings -> Dev cluster -> Platform services -> **Indic voices** (`indic-tts`). It builds `dist/tts-indic/Dockerfile`
(`python:3.12-slim` plus pinned numpy and onnxruntime; it needs network once, on the machine that builds) and pushes it to the
in-cluster registry. The pod runs as user 10001 with a read-only root file system, all capabilities dropped, no service account, and
a PVC at `/models`; default exposure ClusterIP. The image holds no model: `kubectl cp <data>/tts/indic/ta <ns>/<pod>:/models/ta`
for each language. The service has no sign-in, so anyone who can reach it can have text read aloud: keep it on the cluster network
or put an authenticating ingress in front. The Dockerfile was **not built here** (no network authorisation for the base image).

### The text frontend (what the models need)

The models read **characters** (no phonemes, no transliteration inside), one vocabulary per language (67 symbols for Tamil, 128 for
Marathi, 92 for English). Facts measured from the checkpoints:

| | Tamil | Telugu | Malayalam | Kannada | Hindi | Bengali | Marathi | English |
|---|---|---|---|---|---|---|---|---|
| symbols | 67 | 89 | 88 | 104 | 101 | 99 | 128 | 92 |
| ASCII digits | only `0` | none | none | none | many | none | some | all |
| Latin letters | none | 1 | none | none | none | none | 1 | 51 |
| ZWJ / ZWNJ | no | no | no | both | ZWJ | both | both | no |

so: **every digit must be spoken as words**, **Latin words cannot be read by an Indic model at all** (their letters are dropped, a
Tamil sentence with `Wi-Fi` would lose the word), and the characters `ொ ോ ൻ` must be single characters (`compose()`). What IYAKKI does:

1. Symbols: curly quotes removed, `… – —` to full stop and comma, the danda `।` to a full stop, emoji and markup characters dropped.
2. Numbers (`numbers.rs`): ASCII and native-script digits (Tamil `௨௫`, Devanagari `१२`, ...), thousands commas (`1,234`, `12,34,567`),
   decimals (`3.14` -> "three point one four"), `%`, the rupee sign, times (`7:30`). Whole numbers up to 99,99,99,999 are spelt in
   **Tamil, Telugu, Kannada, Malayalam, Hindi and English** (Indian system: thousand, lakh, crore); other languages and longer numbers
   are read digit by digit with the language's digit names. **These tables were written from published number lists and have not been
   checked by a native speaker of each language** (compound forms use the plain "tens + units" reading, without sandhi).
3. Other Indic scripts inside the text are moved letter by letter into the language's script (the blocks are laid out alike);
   letters the target lacks are dropped (Tamil writes voiced and aspirated consonants as the plain ones).
4. Latin words: cut out and read by the **English voice when it is installed**; otherwise written approximately in the language's own
   script by rules (`transliterate_latin`: "net" -> நெட், capitals such as USB are spelt by letter names). This is a rough sound-alike,
   not a dictionary.
5. Pieces: sentences at `. ? !`, long sentences at commas, then at spaces, never inside a word. Limit in characters: 110 for Tamil and
   Malayalam, 130 for Telugu and Kannada, 170 otherwise (the long words of the Dravidian languages make a piece long in sound sooner).
   Pauses: 320 ms after a sentence, 360 ms after a question, 150 ms after a comma, 70 ms inside a cut, 520 ms at a line break.
6. Audio (`audio.rs`): constant offset removed, silence trimmed to 18 ms, **loudness set to -21 dBFS RMS with a 0.89 peak limit** (the
   models' output peaks vary from 0.2 to 1.0 between pieces), 12 ms raised-cosine fades so every piece starts and ends at exact zero,
   then the pauses. No jump larger than the signal's own step at any join (tested).

Not handled: ordinals and dates as words, abbreviations (`Dr.`, `Rs.` beyond the sign), currency other than the rupee sign, English
inside English text (it is read as written), acronyms in the English voice (it reads `USB` as a word).

### The bake-off (measured on the development computer)

AMD Ryzen 9 7940HS (8 cores, AVX-512), Linux, onnxruntime 1.23.2 CPU, two cores (`taskset -c 0,1`), the machine busy (load average
about 17), six sentences per language (the Tamil children's story opening, a greeting, a sentence with a number, a long sentence with
commas, a sentence with an English word written in the language's script, a question), female and male, `fp32`:

| language | audio made (s, 12 sentences) | real-time factor, 2 cores | peak RAM | model size | pipeline RTF (incl. Rust frontend) |
|---|---|---|---|---|---|
| Tamil | 57.4 | 0.36 | 515 MB | 260 MB | 0.39 to 0.40 |
| Telugu | 56.1 | 0.36 | 505 MB | 260 MB | 0.45 to 0.48 |
| Malayalam | 53.3 | 0.36 | 542 MB | 260 MB | 0.36 to 0.38 |
| Kannada | 59.5 | 0.36 | 544 MB | 260 MB | 0.37 |
| Hindi | 45.1 | 0.37 | 501 MB | 260 MB | 0.37 to 0.39 |
| Bengali | 58.4 | 0.36 | 514 MB | 260 MB | 0.39 to 0.44 |
| Marathi | 64.0 | 0.37 | 531 MB | 260 MB | 0.35 to 0.37 |
| English | 53.2 | 0.35 | 577 MB | 260 MB | 0.39 to 0.42 |

* One core: Tamil RTF 0.61. int8 (dynamic quantisation, onnxruntime): FastPitch 207 -> 59 MB and a little faster, but HiFi-GAN's
  quantised convolutions run **slower** on this CPU (RTF 1.86), so fp32 is used. The HiFi-GAN vocoder is about 80 percent of the time.
* The ONNX export is **identical to the PyTorch model**: for four Tamil sentences (both voices) the waveform correlation with the
  original Coqui pipeline is 1.00000 at lag 0 and the lengths are equal; the FastPitch graph matches the PyTorch graph to within 4e-4 (largest difference of any language) at two
  lengths for every language.
* The real-time factor on the Mac mini (Haswell i5, 2 cores / 4 threads) was **not measured**: it is projected to be 3 to 4 times
  slower per core than this computer, i.e. around or above 1.0, which is why the server can live on another machine. Measure it
  with `IYAKKI_INDIC_URL=... cargo test -p iyakki-agent --lib indic::tests::real_server -- --ignored --nocapture`.
* Samples of the same six sentences, from the models (`<lang>_<voice>_<n>_fp32.wav`), from the whole IYAKKI pipeline
  (`pipeline/pipeline_<lang>_<voice>_<n>.wav`) and from espeak-ng (`<lang>_espeak-ng_<n>.wav`), are in `~/.cache/iyakki-tts/samples/`
  with `REPORT.md` and `DETAIL.txt` (durations, peaks, loudness, leading and trailing silence). **No sound was heard here and audio
  quality was not judged**: objective checks only (no clipped samples, no click at a join, plausible durations of 8 to 18
  characters per second). **The owner must listen** (see below).

### How to judge the quality

Listen to the same sentence from each: `espeak-ng` (the robotic baseline), the model's raw output, and the pipeline output. Things to
listen for: (1) the Tamil children's story opening, (2) the number sentence (are the numbers right?), (3) the long sentence (does it breathe at the
commas, does it run on?), (4) the English-word sentence (the word written in the script: is it understood?), (5) a join between two
sentences (any click, jump in volume?), (6) the question (does the pitch rise?). If a number or a word is wrong the cause is the
frontend (`numbers.rs` / `text.rs`) and is fixed there; if the sound is bad everywhere the cause is the model.

### Licences of the voices (read before you use or pass them on)

* The **code** of AI4Bharat/Indic-TTS is MIT (`LICENSE.txt`, copyright 2023 AI4Bhārat; GitHub reports MIT). IYAKKI's server and Rust
  code are original.
* The **model files** (the `v1-checkpoints-release` assets): **no licence is stated** by AI4Bharat in the repository README, the release
  page ("Just creating a release to upload checkpoints.") or the paper (it says only that the models are open-sourced on the Bhashini
  platform). A third-party Hugging Face repository labels the same files MIT; that is not AI4Bharat's statement. **This is unclear, so
  nothing of it is bundled, shipped or fetched by IYAKKI**; the engine is off until an administrator installs a bundle they exported
  themselves, and the page says so before every download. Ask AI4Bharat (ai4bharat.org) to confirm the licence of the checkpoints
  before you redistribute exports.
* The **training data** is the IIT Madras Indic TTS database, under its end-user licence
  (`https://www.iitm.ac.in/donlab/indictts/downloads/license.pdf`, read in full): perpetual, worldwide, royalty-free, sub-licensable;
  you may copy and modify and own your derivative works and distribute them, but must keep the notice "COPYRIGHT 2016 TTS Consortium,
  TDIL, Meity represented by Hema A Murthy & S Umesh, DEPARTMENT OF Computer Science and Engineering and Electrical Engineering,
  IIT Madras. ALL RIGHTS RESERVED" and a third party to whom a derivative work is sold must be told it contains open components and may not
  further sell, lease, license or sub-license it. Not copyleft, not non-commercial; the last restriction matters if you hand voices on.
* The export tool uses AI4Bharat's Coqui TTS fork (MPL-2.0) and PyTorch (BSD-3-Clause) **only on a PC, to convert**; neither is linked
  into IYAKKI or shipped. The server needs numpy (BSD-3-Clause) and onnxruntime (MIT). Details: `docs/LICENSES.md`.

### English and other options

For Indian English the same engine has a voice (`en`). If you want a different English voice, Kokoro behind any OpenAI-compatible
speech server remains an option (Settings -> Voice -> Text-to-speech, an `/v1/audio/speech` address); it was not evaluated here.

### If a pure-Rust runtime is wanted later

Export splits cleanly: `fastpitch.onnx` takes `tokens [1,T] int64`, `speaker [1] int64`, `scale f32` and returns `mel [1,F,80]`;
`hifigan.onnx` takes `mel [1,F,80]` and returns `wav [1,1,N]` (256 samples per frame). The only dynamic shape step is the length
regulator (`repeat_interleave`), which a runtime without data-dependent shapes would do outside the graph, by splitting
FastPitch in two. Before adding `tract` (Apache-2.0 / MIT) to the build, measure it against onnxruntime on the Haswell Mac mini; a result
under real time with a margin for the vocoder is what would justify it.

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

**Indic voices.** Verified: the ONNX export equals the PyTorch model (waveform correlation 1.00000); the server was run for real with
eight languages and the whole Rust pipeline produced WAVs for every sentence with no clipping and no click at any join; the text
frontend, number words, pieces, audio joins, model manager (download, resume, hash mismatch, delete, folder import) and the engine
adapter (including the fallback to the next engine) are unit-tested with fake files, fake servers and a fake engine script; the
Settings page at 1280x720, 1920x1080, 1024x600 and 800x480. **Not verified:** how any of it sounds (nobody listened), the number
words by a native speaker, the real-time factor on the Mac mini, the Dockerfile build and the k3s deployment (the manifest is linted,
not applied), a download from a real host, the licence of the checkpoints (unclear, above).

**Word timing from the sound and remote Lemonade.** Verified against fake servers on loopback only: a fake Lemonade answering a
canned WAV and a canned whisper.cpp `verbose_json` (misheard, missing and extra words, Tamil), a server that fails, one that never
answers and a slow one, Kokoro-FastAPI's captioned answer, the reader switching from the estimate to aligned times while a piece
plays, and the yes card for a server on another machine. What Lemonade sends was read from its source (lemonade-sdk/lemonade,
lemonade-sdk/Kokoros, ggml-org/whisper.cpp `examples/server`, October 2026). **Not verified:** a real Lemonade, a real whisper
(how well its word times follow Kokoro's voice, and how often it hears Tamil well enough to match), and how the aligned highlight
looks and sounds together.
