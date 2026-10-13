# MEDIA search, the catalog index, downloads and credentials

This page describes how IYAKKI finds music, videos and pictures ("play a kutthu song"), how the same index serves the
MEDIA app and the chat, how files are downloaded into the library, and how credentials are kept out of the chat.
Code: `crates/iyakki-agent/src/mediaindex.rs`, `mediadl.rs`, `mediaqueue.rs`, `mediaserve.rs`, `secretreq.rs`, `credguard.rs`; screens in
`crates/iyakki-shell/src/apps/media.rs`, `inbox.rs`, `chat.rs`.

## Why not DuckDB

The knowledge base already has an in-house hybrid engine (`kb.rs`: BM25 scoring, cosine over dense vectors, Reciprocal
Rank Fusion, MMR; `embed.rs`: `TextEmbedder`, the hashed default and the provider embedders). DuckDB would add a large C++
build and about 40 MB, and its full-text and vector extensions are downloaded at run time, which does not fit a static
musl, offline, auditable image. The media index reuses the same ideas and the same embedder plumbing instead.

## What is indexed

One index per person (`media-index-<user>.json` plus `media-index-<user>.vec` in the services folder), built from the
library scan (songs, videos, podcasts) and from pictures (jpg, png, webp, gif, recognised by their first bytes) found in
the library folders and in `/home/<user>/Pictures`. Only what the account may read is scanned.

| Field | Where it comes from |
|---|---|
| title, artist, album, year, genre | tags (ID3v2/v1, Vorbis, FLAC, MP4) with file-name fallback; the year of a picture is its folder year or its modified time |
| kind | `song`, `video`, `image`, `podcast` (a podcast is an audio file whose genre, folder or name says so) |
| types | found automatically in the genre tag, the folder and file names and the title, through a vocabulary of synonyms; the person's choice replaces them (known words under their canonical name, any other word as written) |
| languages | words (tamil, hindi, telugu, ...) in tags and paths, and the script of the title (Tamil, Devanagari, Telugu, ...) |
| tags | the person's own words; a word that is a known type is stored under its canonical name |
| rating | 0 (unrated) to 5 stars, set by the person |
| plays, last played, added, duration, bpm, size, mtime, source address | counted and kept; `bpm` is stored when set but nothing detects it yet |

The vocabulary (`mediaindex::TYPES`) maps spellings to canonical types, for example `kuthu | kutthu | koothu | kuththu | குத்து`
to `kuthu`, `melody | melodious | மெலடி` to `melody`, `beats | beat | dance | party | dj` to `beats`, and also devotional,
ghazal, rock, metal, classical, carnatic, hindustani, hip-hop, lofi, pop, folk, jazz, blues, edm, sad, romantic,
instrumental, remix, sufi, bgm, motivational, lullaby, podcast, and for videos and pictures action, horror, cartoon
(animation, anime), comedy, drama, sci-fi, documentary, kids, thriller, family, travel, nature. The vocabulary is a list of
*suggestions and synonyms*, never a gate: any other word a person types as a category or tag (`road trip`, `bollywood
90s`) is kept as typed (lowercase) and searched like any other word; it shows on the Types page with its count. Each type carries a few "mood words" that go into the
descriptive text the dense vector is made from.

A rescan keeps the person's rating, tags, types, plays and source address for files that are still there; a file whose size
or modified time changed is read again; files that are gone are dropped (files added by a download stay while they exist).

## How a search works

1. **Filters first.** Kind, types, languages, minimum rating, year range and artist narrow the candidates before any scoring.
2. **Inverted index.** `term -> postings (document, weighted term frequency)`. BM25 (k1 1.2, b 0.75) with field boosts:
   title 3.0, artist 2.0, types/tags/genres 2.0, album 1.0, path 0.5. A word with no exact entry tries entries it begins.
3. **Dense vector.** Each item has one vector made by the configured `TextEmbedder` from a sentence such as
   "Vaathi Coming by Anirudh, kuthu tamil, mood energetic fast dance ..., song". Vectors are normalised and compared by a
   flat dot product over the filtered items. They are optional: they are skipped when the embedder id differs from the one
   that made them (the words still work), and rebuilt in the background by `embed_all`.
4. **Fusion.** The top 200 of each list are fused with Reciprocal Rank Fusion (k = 60). Ties go to the better rated, then the
   more played item. A sort hint (`latest`, `top rated`, `most played`) reorders the matches.
5. **`Query::explain`** fills `Hit::explain` with the BM25 score and rank, the cosine and rank and the fused score (used by tests).

## Query grammar

`Query::parse` is a small rule-based parser (no model). Everything it recognises becomes a filter or a sort hint; what is
left is the text for the word and vector search.

| You type | It means |
|---|---|
| `kutthu song`, `kuthu`, `குத்து` | type kuthu |
| `melody songs by anirudh above 4 stars` | type melody, artist contains "anirudh", rating at least 4 |
| `latest tamil beats` | language tamil, type beats, newest first |
| `videos`, `movies`, `clips` | kind video |
| `photos from 2024`, `pictures 2019` | kind image, year 2024 |
| `songs after 2010 before 2020` | year range |
| `top rated`, `best`, `favourite` / `most played`, `popular` / `latest`, `new`, `recent` | sort hints |
| `rated 3`, `3 stars`, `over 4 stars`, `at least 4 stars` | minimum rating (the number itself counts) |
| `hip hop`, `lo-fi` | two-word types |
| `podcast` | kind podcast |
| anything else, e.g. `vaathi coming` | words and vector |

Filler words (`play`, `a`, `some`, `song`, `please`, ...) are ignored. Examples that work in the MEDIA search box (`/`) and
in the chat ("play a kutthu song", "play melody by anirudh above 4 stars").

## The MEDIA app

`/` search in plain words, with the understood filters shown as chips; `1`-`5` rate the chosen track and `0` clears (on pages
without tracks the digits jump to that page); `t` edits types and tags; `o` cycles the sort; `i` shows details, including the
address a file was downloaded from; pages **Types** (kuthu, melody, beats ... and languages, with counts), **Top rated** and
**Photos**; `D` adds addresses to the download queue and the **Downloads** page shows it (below). The look matches RADIO: a glowing now-playing card with a pulsing orb, type chips, stars, round
transport buttons, a progress bar with elapsed and length, and a level display. The level display is cosmetic: there is no
audio analysis, the bars move while something plays and are still otherwise. Kuthu is drawn hot magenta, melody cyan and
beats amber.

## The chat

Tools (offered when the role has the MEDIA app and the command line policy is not off): `media_search`, `media_play`,
`media_stop`, `media_rate`, `media_tag` and `media_downloads` (no approval: they only read the person's library and download
queue, play files inside their read scope and change their own ratings and tags) and `media_download` (asks first, medium
risk: network and write; it only *queues* the download and answers at once with `queued as #N`, the state is read with
`media_downloads`). Results show
an id, title, artist, types, rating and length, never a file path; titles come from files and are marked untrusted. The
player and the play queue are shared (`MediaPlayer` in the agent crate), so music started from the chat shows in the MEDIA
app. A queue of several ids is handed to an external player as one playlist file, so that player moves on by itself; when
MP3 plays inside IYAKKI (below) the first file starts and the MEDIA app, when it is open, carries on with the rest.
`mediaindex::suggest_tags(items, chat)` asks the model to sort songs into the known types and returns proposals; nothing is
applied until the person accepts (`apply_proposals`). There is no app button for it yet.

## Downloading, the queue and categories

`D` in MEDIA (or the `media_download` tool) adds one or more direct addresses of songs, videos or pictures to the person's
**download queue**. The form has: *Address(es)* (several: separate with spaces, or paste a list, one per line), *Type /
category*, *Tags*, *Rating* and *Folder*.

**Categories are free text.** Type any words, comma separated (`action, horror`, `kuthu`, `my road trip`). Under the field are
clickable example chips that fill it (a second click takes the word out again; the keyboard: Ctrl+Left/Right moves along the
chips, Ctrl+E picks one): for videos action, horror, cartoon, comedy, drama, sci-fi, documentary, kids, thriller, romance;
for songs melody, kuthu, beats, devotional, classical, rock, hip-hop, lofi, ghazal; for photos family, travel, nature. The
chips follow the kind of file as soon as the address names it (`.mp4` shows only the video row); until then all three rows
are shown. A word the vocabulary knows (`kutthu`, `bhajan`, `anime`, `tamil`) becomes a type of the item under its canonical
name, any other word becomes a tag; nothing is rejected (`mediadl::Choice::from_words`, `MediaIndex::set_types`). The same
words work in the chat tool (`type` accepts a comma separated list) and in the `t` form.

**The folder is automatic by kind of file**: songs to Music, videos to Videos, pictures to Pictures (among the library
folders the account may write to). It is decided from the address ending first (so the form can say `Folder: Videos (auto)`
at once) and, for an address without an ending, from the first bytes of the file and the content type once the first
response arrives (`mediadl::Target::Auto`). To choose a folder, Tab to the *Folder* line and use the arrow keys: the first
choice is *Automatic*, the others are the writable library folders. The person never needs to know the kind.

**The queue** (`mediaqueue.rs`) runs first in, first out with at most **two downloads at a time** per person. Entries are
numbered (`#1`, `#2`, ...). The **Downloads** page (the last page, key `0`) lists each entry: *Queued*; *Downloading* with a
progress bar, bytes, speed and the time left when the server said the size; *Succeeded* with the file name, folder and size and
**Play** / **Show in library** buttons; *Failed* with the reason in plain English and **Retry**; *Cancelled*. Keys: Enter plays
a finished download (or tells why one failed), `x` cancels (a running download stops at its next chunk and its partial file is
removed), `r` retries (the entry goes to the end of the line), `d` removes an entry from the list (a running one has to be
cancelled first; the file, if any, stays), `c` clears the finished ones, `f` shows a finished file in the library. A small chip
in the MEDIA header shows `Downloads: 1 running, 2 queued` while there is work and flashes `Download succeeded: name` or
`Download failed: name` for a few seconds when one ends; a click on it opens the page. The inbox still gets one note per
finished or failed item.

Failure reasons are one of: *Not a media file* (a web page, an unknown format), *Too large* (over the size cap), *Host refused*
(an address the account may not reach, also at a redirect), *The server refused or failed* (HTTP 4xx/5xx), *Disk full*,
*Outside the write scope*, *Folder missing* and *Network error* (anything else, with the detail). The list is saved per person
in `media-downloads-<user>.json` in the services folder and survives a restart; an entry that was queued or running when the
program stopped comes back as *Failed: Interrupted* so that one key retries it. A playlist (`.m3u`, `.pls`) or podcast feed
address is expanded into its media addresses (at most 50) that join the queue.

Rules for every download, in order: the address must be http or https; the host must be one the account may reach (checked
again at every redirect); known video pages (youtube.com, vimeo.com, ...) are refused with a suggestion to use the browser;
the folder must be one the account may write to; a web page is refused whatever the address or content type says; the first
bytes must be a recognised audio, video or picture container; the size is capped (default 2 GB, `media_download_max_mb` in the
settings file); the file is written as `name.part` and linked into place so that nothing is ever overwritten (`name (2).mp3`).
The audit log gets `media.download`; the finished file is added to the index with the chosen types, tags and rating and its
source address.

Not done: scraping pages, and the optional `yt-dlp` extractor. It is not implemented. If it is added later it must stay off
by default behind a Settings switch with its own BOUNDARY row; the address is passed as one argument and the output kept in
the target folder. Resuming a half-done download is not done either (a retry starts again). **You are responsible for what you
download**: only download what you have the right to keep.

## Samples

Every image carries three small files made by us (no third-party material; CC0): `IYAKKI Sample Melody.mp3` (26 s, genre
melody), `IYAKKI Sample Beats.mp3` (20 s, a kuthu-style drum loop, genre kuthu; both with ID3 tags, artist IYAKKI, album IYAKKI
Samples) and `IYAKKI Sample Video.webm` (12 s, 640x360, VP9 + Opus). They live in `dist/samples/` (with the script that makes
them and a README/LICENSE) and `board/iyakki/post-build.sh` installs them, read-only, in `/usr/share/iyakki/samples/` of the
default and of the `web` image. MEDIA scans that folder in addition to the person's own: it appears on the Folders page as
**IYAKKI samples** (built in, read-only, tagged `sample`), the files show in Tracks and Videos on a fresh install, "play a
kutthu song" finds the beats sample, and they can be read whatever the account's read scope says (`media::may_read`). The folder
is never a place to download to and `d` on it only explains that it cannot be removed. `IYAKKI_SAMPLES_DIR` points a development
machine elsewhere (empty = none).

## Playing

* **MP3 plays inside IYAKKI**: the same in-house decoder as the RADIO tile (`nanomp3`, WAV to the sound command of Settings ->
  Voice). The shared player (`media::SystemLauncher`) takes the MP3 route first, so the default image, which has no other
  player, plays the sample melody. A file that cannot be decoded, or a missing sound command, is reported at once.
* **Other audio and video** go to an installed player (mpv, VLC, ffplay or mplayer) in its own window.
* **Video on the web image** (when no such player is installed and the embedded web engine helper exists,
  `webengine::embedded_helper()`): the MEDIA page area becomes the video. See the next section.
* On an image with neither: `Video needs the web image or an installed player (mpv, VLC, ffplay).`

### Video on the web image

The engine's helper only navigates to `http` and `https` addresses (`webengine::check_url`), and a `file:` address would let a
page read whatever the helper process can reach. So the shell never gives the engine a file path. For one playback it starts
`mediaserve::LocalServe`, a tiny server that:

* listens on **127.0.0.1 only**, on an ephemeral port, and closes when the playback ends;
* serves exactly two paths, both starting with a random 128-bit token read from `/dev/urandom`: `/<token>/` (a generated player
  page with `<video controls autoplay>`) and `/<token>/media` (the one chosen file, opened before the server started, with
  `Range` requests so the player can seek); everything else is a 404 that gives nothing away;
* answers only `GET` and `HEAD` to a `Host` of exactly `127.0.0.1:<port>` (a rebound DNS name gets a 403), with
  `Cache-Control: no-store`, `nosniff`, `Referrer-Policy: no-referrer` and a Content-Security-Policy that allows only that media
  and the page's own script.

The helper is started with `allow_hosts = [127.0.0.1]`, and the shell vets every address the engine reports again, so the page
cannot navigate away. The account's read scope is checked before anything starts (`media::may_read`). The page's script handles
Space (pause or play), Left and Right (seek 5 s) and `m` (mute); Esc, or the Stop button, ends the playback, stops the engine
and closes the port. A loopback server with an unguessable path was chosen over `file:` URLs because the engine then needs no
file-system access at all, the same code path as every other page, and no change to the helper's navigation rule. Cost: a
local port is open while a video plays (reachable only from this machine, and only with the token). The WPE helper plays
VP9 and Opus (WebM); H.264/AAC/MP3 need the optional `WEB_H264=1` build (docs/FULL-BROWSER.md, "Codec policy"), so the samples
are WebM.

## Credentials stay in vaults

* The base prompts (chat and agent) tell the assistant never to ask for passwords, API keys, tokens or cookies in the chat,
  and to say that credentials go in the vault, referred to as `vault:<name>` (system vault) or `uvault:<name>` (the person's
  own vault).
* Tool `vault_request(name, purpose, kind)` does not take the secret. It registers a request (`secretreq.rs`) and waits. The
  request appears in the INBOX under **Approvals** as "needs a secret" with an *Enter secret* action, and the private form
  opens by itself in the chat while that chat's run waits. The person types a vault name (suggested `svc.<name>`) and the
  secret in a masked field; the value goes straight into their user vault (`Services::answer_secret_request`). The tool
  result is only `stored as uvault:<name>` or `cancelled`. The value is never in the model's context, the tool log, the chat
  history, the run record or the audit log (the audit log records that a secret with that name was stored). A locked vault
  is reported at once. If nobody answers, the request is withdrawn after nine minutes.
* The chat input guard (`credguard.rs`) looks at what is typed before it is sent: `sk-...`, `ghp_...`/`github_pat_...`,
  `xoxb-...`, `AKIA...`, `AIza...`, JSON web tokens, private key blocks, `user:password@host` addresses, and values after
  `password:`, `api key:`, `token=`, `Bearer ...` that look random. A message that is only a credential is not sent at all;
  one with a question around it is sent with the value replaced by a placeholder. A notice offers *Store in vault (v)*, which
  opens the same masked form with the value filled in. Ordinary text such as "my password is hard to remember" is left alone.
  It is a heuristic and can miss unusual formats.
* A media server, cloud provider, website login, MCP token or NAS login that needs a credential should use `vault:` or
  `uvault:` references. Nothing in MEDIA needs one yet.

## Benchmark

`cargo test -p iyakki-agent --release --lib benchmark_fifty -- --ignored --nocapture`

50,000 items (songs with genre, artist, year, ratings on every third item), 64-dimension vectors from the hashed embedder,
release build, on a shared 28 GB development machine while other builds were running:

| Step | Time |
|---|---|
| build the index from a scan (`sync`, inverted index included) | 280 ms |
| make 50,000 vectors (hashed) | 57 ms |
| search (7 phrases such as "kutthu song", "melody songs by anirudh above 4 stars", "moon river", 5 rounds) | average 1.6 ms, worst 3.5 ms |
| save (30 MB: JSON items + binary vectors) | 84 ms |
| load | 150 ms |

The search target was under 20 ms at 50,000 items; the vector side is a flat dot product over the filtered items, so no bucket
index was needed. With the default 256-dimension hashed vectors the vector part takes about four times longer. A real
embedding model costs one call per distinct search phrase (the MEDIA app caches the rows of the current search), and its
vectors for a large library are made in the background in batches of 256.
