# LAB: try picture and vision models

LAB is a tile in the **Work** section (next to DATA, DASH and FLOWS: it is a workbench you try things on, while Home keeps the everyday
assistant). It is for finding out which picture-making model and which picture-reading model suit you, and for comparing two of them
on the same input. It uses the providers and models you already have in Settings -> Models and adds none.

Three tabs, `F2` / `F3` / `F4` (or `p` / `v` / `d` when no box has the keyboard), and a **log** along the bottom of each.

## Picture (make a picture)

* **Model**: every picture model of every enabled provider, this computer's first. Each row says the provider, whether it is remote,
  the download size and the model's own size and steps when its server says them (a Lemonade server's `image_defaults`: SD-Turbo
  512x512 in 4 steps, SDXL 1024x1024 in 20). A Lemonade picture model that is not downloaded is not offered; the tab says how many are
  known and points to Settings -> Models.
* **Prompt**, the TALES looks as **Style** buttons that add a phrase to the prompt (watercolor, photo, cinematic, anime, pencil; the
  wording is a copy of TALES' short looks), **negative prompt** (shown only for a server that takes it: a Lemonade server sends it as
  `negative_prompt`; for the others the tab says there is none), **size** (64 to 2048, set from the model's own and kept to multiples
  of 8), **steps** (1 to 100, only for a server that takes them, filled with the model's own) and **seed** (empty means a random one,
  which is shown with the result and can be reused with **Reuse seed**).
* **Generate** runs on a background thread with a progress line and **Cancel** (`Esc`). The result shows the picture, the time taken,
  the bytes, the size it really has, the seed, the steps the server used, the model and the provider.
* **Compare (one after the other)** draws the same prompt, seed and size with the model and the **Compare with** model, one
  after the other (they do not run at the same time; the screen says left ran first), and shows both with their times. If the second
  fails, the first stays and the failure is shown with its reason.
* **Save picture** writes the picture shown to `Pictures` in your home (only where your account may write). **Copy prompt**.
* **This session**: the results kept so far, newest first; click one to look at it again.

## Vision (picture understanding)

* **Model**: the chat models marked as vision (a Lemonade server's `vision` label, or a name such as `gpt-4o`, `gemma3`, `qwen-vl`,
  `llava`). **any chat model** adds the other chat models with a warning: they are sent the picture and the server decides; if it
  refuses because the model cannot take images, the answer pane says so in words (the server's reason is quoted) and nothing is made
  up. A model that can never see pictures (a picture-making, speech, embedding or reranking model) is refused before anything is sent.
* **Picture**, from one of: **File** (type a path or **Browse**; PNG, JPEG, GIF, WebP; only files your account may read; over
  25 MB or 64 megapixels it is refused), **Camera** (one frame; it asks every time, the red "camera in use" sign shows while the
  camera is used, and the frame stays in memory), **Screenshot** (the IYAKKI screen as drawn, this tile included; it asks every time and
  warns it may show private information), **Clipboard picture** (the picture last copied inside IYAKKI).
* A picture over 1024 pixels on a side, or whose PNG would weigh more than 3 MB, is made smaller before it is sent. The tab shows what
  was sent: *Sending 1024x512, 412 KB (was 3000x1500, 3.1 MB: made smaller to be sent)*.
* **Question** box and quick questions (*Describe this picture*, *Read the text*, *What is wrong with this screenshot?*, *List the
  objects*). **Ask** shows the answer as it arrives when the provider streams (otherwise in one piece, and it says so), with the time
  and the token counts when the provider reports them (never guessed).
* **Compare two models** asks both the same picture and question, one after the other, side by side.

## Video

* **Video understanding**: pick a video file, 4 to 16 frames, *evenly spaced* (the middle of equal parts of the video) or *every k
  seconds* from the start, and a question. LAB takes the frames out with `ffmpeg`, shows them with their times, sends them (each at most
  768 pixels on a side) with the question to the vision model, and shows the answer. The question sent says the pictures are frames
  of one video, in order, and at which times.
* **ffmpeg is a vendor tool you install, never shipped** (Store -> Setup -> Media tools -> Media player; the same program the MEDIA
  player uses; one found on the `PATH` also works). Without it the tab says so in a card with a button that opens that setup; it is
  not an error dialog and nothing is sent. No other decoder is used for video here.
* **Video generation**: offered only when a provider's model list has a model with a video label or recipe. Today none does (Lemonade
  has no video models and no video call), so the tab shows one line: *No video generation model is available from your providers.*
  and where models are installed. If a provider ever lists one, the line says it was found and that LAB has no call for it yet; no
  request is sent and nothing is faked.

## What goes where

* The button area says where each run goes: *Runs on home (this computer)*, or *Runs on gpu (remote, the network)*. A provider on
  another computer asks on a card before the first use **in this session** (once per provider); the answer is kept in memory only,
  never in the settings file, and a new LAB asks again. No sends nothing.
* **Log** (every tab): one line per call, time, tab, provider, model, a request summary (sizes, seed, a shortened prompt, never
  headers or keys), the HTTP status (for a question, 200 on success or the code in the server's error), duration, bytes, errors and
  notes (a server that drew at another size, steps asked for, a picture made smaller, an answer that came in one piece, token counts
  not reported). Server error text has the key removed. **Copy log** and **Clear log**; the lines can be selected with the pointer.
* **Memory and disk**: the session keeps at most 16 pictures and 48 MB of them (the oldest go first, the newest is always kept);
  the log keeps 300 lines; answers, frames and inputs live in memory. **Nothing is written to disk except Save picture.**
* **Cancel** stops waiting at once and drops the answer; a request already sent to a server is not recalled and may keep the server
  busy until it finishes.

## Keys

`F2` `F3` `F4` tabs · `Tab` / `Shift+Tab` next box · `Ctrl+Enter` run · `Esc` cancel, leave a box, go back · with no box focused:
`g` generate, `c` compare, `m` model, `s` save (Picture); `f` file, `b` browse, `k` camera, `x` screen, `l` clipboard, `r` ask (Vision);
`+` `-` frames, `b` browse, `r` run (Video). Everything is also a button. The controls scroll (wheel, `Up`/`Down`) while the buttons,
where the run goes and the progress stay in view.

## Where the code is

`crates/iyakki-agent/src/lab.rs` (catalogue, caps, frame maths, log, backends, ffmpeg arguments; tests in `lab_tests.rs`),
`crates/iyakki-shell/src/apps/lab.rs` (state, keys, jobs), `lab_ui.rs` (drawing), `lab_pic.rs` (preparing pictures), `lab_tests.rs`.
The policy entry is `App::Lab` (Admin, Operator and User; not Guest); in cloud mode it stays (it only needs the providers).

## Tried and not tried

Tested with fakes: a fake picture backend that answers with a tiny PNG, a fake vision backend that streams, a fake frame source, a
fake provider behind the real backends (request bodies, tokens, keys kept out of messages). **Not tried:** a real picture model, a real
vision model, a real camera, the real screen capture on a running desktop, a real `ffmpeg`, streaming over the real streaming
transport. The steps and size overrides depend on `ImageParams` (`steps`, `size`) in `providers.rs`.
