# Assistant robustness and speed

The chat assistant has to be a personal assistant you can lean on: it must use the tools it has, and it must answer fast.
This page says what was wrong, what was built, how to measure it, and what is still slow.

## What went wrong

With a small local model (qwen3:8b through Ollama) in a chat whose history already held several answers like
*"I cannot access external websites"*, the model kept saying *"I cannot access external services… my capabilities are confined to
the tools described earlier"* and *"I cannot take screenshots of my own interface"*, although `web_research`, `weather`,
`stock_quote`, `browser_*`, `radio_*` and the media tools existed. Earlier, models asked *"Would you like me to fetch…?"* instead
of doing it. Small models copy their own earlier words, lose a 2,500-token system prompt in the middle of a long request, and
sometimes print a tool call as text instead of calling it.

## What is built

All of it applies to **Agent mode** (Ctrl+G) and is on by default there. The code is in `crates/iyakki-agent/src/`:
`assistrun.rs` (the loop), `toolblock.rs`, `assistant_guard.rs`, `toolrepair.rs`, `screencap.rs`.

### 1. Capability reminder, every turn

`toolblock::reminder` writes a block like

```
[TOOLS AVAILABLE NOW]
web: now, web_research, web_search, web_fetch, web_feed, web_links, weather, stock_quote, browser_page
browser: browser_status, browser_open, browser_snapshot, ... (not loaded yet: call more_tools)
radio: radio_play, radio_search, ... (not loaded yet: call more_tools)
...
Switches: Agent ON; browser control interactive (full engine installed); screen capture available (asks every time).
These tools exist right now. Earlier statements in this conversation that you cannot do something are outdated if a tool above
does it. If a tool is missing, say exactly which one and why (browser_status explains the browser). Read-only lookups need no
permission: just call the tool.
```

It is generated from the live tool registry of this account (so a tool the account does not have is never listed), grouped by
purpose, about 380 to 400 tokens for an administrator's 78 tools (long lists are cut with `+N more`). It is appended to the
**latest user message of the request only**, which is the end of the context and works with every model template. It is
never shown in the transcript, never saved in the chat, never in a run record. The system message stays byte-identical between
turns, so a local server can reuse its cache of that prefix.

### 2. Denial guard

After the model's final text (no tool call), `assistant_guard::detect_denial` looks for a refusal ("I cannot access (external)
websites", "I don't have browser access", "I can't take screenshots", "I cannot interact with external services", "my tools are
limited to…", "I don't have real-time data", about 70 phrasings) **and** a tool in the registry that does what the request or the
refusal is about (website, gemini, news, weather, stock, screenshot, radio/play, remind/calendar, files, this page). Then:

1. nothing is shown; a step `assistant_check` appears in the chat ("The model said it cannot do this, but these tools can (…)");
2. the model is asked again, once, with its own refusal and a `SYSTEM NOTE` that names the tools and says to quote tool errors
   instead of claiming "no access";
3. if the second answer still refuses, it is shown with the line *"This model may not be using its tools well; try another model
   (Ctrl+P) or a new chat (Ctrl+N)."*

True refusals are left alone: when browser control is off for the account (only `browser_status` exists), when the full web engine
is not installed, after a denied or unanswered approval, when no tool matches, and any answer longer than 900 characters.
`detect_ask_first` does the same for *"Would you like me to fetch/search/open…?"* when the action is read-only (writes, sends,
installs, purchases are never pushed) and no tool has run yet in this run.

The guard also holds back the live view of text that starts like a refusal ("I cannot…", "Sorry…", "Unfortunately…") so a
retried refusal never flashes on the screen.

### 3. History hygiene

What the model is sent is `assistant_guard::condense(history)` (the saved chat is not touched):

* an old assistant answer that is a refusal contradicted by today's tools is replaced by one line saying it was wrong;
* the guard's own `assistant_check` steps are removed;
* tool results from before the last two questions are shortened (failures become "an old failure, left out");
* the request is cut to about 6,000 tokens: the oldest whole exchanges go, and what was cut becomes a rolling summary
  (one line per exchange, newest first, 1,500 characters at most) added to the system message and stored in the chat file
  (`Session.summary`). The summary is **extractive**, written by IYAKKI, not by the model: it costs nothing and cannot fail.
  A model-written summary is not built;
* during a run the oldest tool results are shortened first when the request outgrows the cap.

### 4. Tool-call repair and the text protocol

`toolrepair.rs` turns what weak models write into a proper call:

| the model wrote | IYAKKI runs |
|---|---|
| ```` ```json {"name": "web_search", ...} ``` ```` as the answer, or `<tool_call>…</tool_call>` | the call (also several at once) |
| `{'query': 'x',}`, `{query: "x"}`, `True/None`, curly quotes, a cut-off object | parsed JSON |
| arguments as a string `"{\"query\": \"x\"}"`, or wrapped in `{"arguments": …}` / `parameters` / `input` | the argument object |
| `Web.Search`, `web-search`, `functions.web_search`, `WebSearch`, `search`, `get_weather`, `play_radio` | the registry's name (only if that tool exists) |
| `"limit": "5"`, `"metric": "yes"`, `"add": "kuthu, melody"`, a number for a string | the schema's type |
| a missing required argument, a wrong enum value, an unknown tool | an error that names the problem and shows the valid form: `missing required argument 'symbol'. Valid form: stock_quote(symbol: string*)` (or the list of tools that exist) |

**Text protocol.** If the server answers that the model has no native tool calling (Ollama: *"… does not support tools"*), the run
switches, with a visible step, to a protocol in the system message: the model prints

````
```tool
{"name": "weather", "arguments": {"place": "Chennai"}}
```
````

and IYAKKI runs it; results come back as user messages (`Result of weather: …`). Models whose server silently ignores the tools
(they answer in prose) are not detected by an error: for those the denial guard and the repair of printed calls help.

**Loops, time and Stop.** The same call giving the same answer three times in a row is cut off with a `LOOP:` result and the
model is told to answer now without tools; five times stops the run with a message (browser tools are exempt: a page changes
between snapshots). Every tool call has a time limit (120 s; 300 s for web research and browser tools; 600 s for commands and
secrets) and the model call and every tool run on their own threads, so **Stop answers within about 50 ms** even while a
provider is thinking or a tool is stuck. Every failure ends in a visible message (`Failed`, `Stopped`, a tool error), never a
silent stop.

### 5. `screen_capture`

A picture of the person's current IYAKKI screen, shown in the chat like `show_image`. Only the shell can see the screen, so the
tool puts a request in `Services::screen` and the shell answers on its next drawing with the frame it has just drawn (nothing
is blanked out). The tool exists only while a shell with a screen is drawing (window, framebuffer or the web front end all draw
through the same shell). It asks **every time** in the chat: *"Let the assistant take a picture of your IYAKKI screen? It may show
private information."* with **Approve**, **Allow for 5 minutes** and **Deny**. "Allow for 5 minutes" skips the question for that long
for that person (not for the rest of the run). Audit entries: `screen.capture` (size only), `screen.capture.grant`.

### Links in the transcript

Answers and tool results hold addresses; the chat makes them links (`crates/iyakki-shell/src/links.rs`, `mdtext.rs`,
`apps/chat_view.rs`, `apps/chat_links.rs`). A link is one underlined span even when it wraps over lines (one hit region per
line), shows the hand pointer, and on hover or keyboard focus the status line under the transcript shows the **full target**.
Tab and Shift+Tab walk the links (with an empty box), Enter opens the one in focus, Esc leaves them.

* **Found:** markdown `[text](url)` (titles, balanced parentheses, `<url>`), `http(s)://`, `www.`, `mailto:`, `tel:`, `sms:`, `geo:`,
  bare `domain.com/path`, also in tool-result blocks. Code spans and fenced code are never linked. Trailing punctuation and an
  unbalanced `)` are left out; an address over 2000 characters is not a link.
* **Opened only by the person.** Drawing, waiting, a model message or an Autopilot turn open nothing. A single click opens after
  the double-click time (about 0.45 s); a drag selects text and never opens; a double click selects a word.
* **By scheme:** `http(s)` -> the IDE Browser tab in the IDE (the click is the consent; local and own-cluster plain http is fine),
  a new tab of the BROWSER tile elsewhere (Back returns to the chat). `mailto:` and `tel:`/`sms:` -> copied with a note (there is
  no mail program or dialer). `geo:` -> an OpenStreetMap page; map links also get Copy coordinates / address. `file:` and
  workspace paths (`src/main.rs:42`, `./deploy/app.yaml`, `x.rs#L7`) -> the editor at the line, only inside the account's file
  scopes (the workspace in the IDE), `..` and anything outside refused with a plain reason. `ssh://`, `vnc://`, `rdp://`, `ftp://`
  and so on -> a card saying IYAKKI cannot open it, with Copy. Unknown schemes -> the card, copy only. `javascript:`, `data:`,
  `vbscript:`, `blob:`, `about:` (except `about:blank`) -> never opened: plain text with a note.
* **Marks:** an amber underline and badge when the visible text reads like another address than the target, for plain `http` to
  anything but this computer or a private network (`localhost`, `10.x`, `192.168.x`, `.svc`, `.local`), and for `xn--` hosts (the
  status line shows the decoded host). The status line names the reason.
* **Right-click:** Open, Open in new browser tab, Open in IDE browser (IDE only), Copy link address, Copy link text, Copy
  coordinates / address (map links), and Open in the system browser in a desktop window only: `xdg-open` with the address as one
  argument through the `wsexec` runner, `http`/`https` only, asked once per session (BOUNDARY.md).
* **Prompts:** the model is told to give links as markdown with a human label and the full URL, to put stable map links
  (`maps.apple.com/?q=Name`, OpenStreetMap with coordinates) for place answers and the address as text beside them, and that it
  cannot open links itself.

Tests: `links::tests::*` (detection, destinations, schemes, warnings, map text) and `apps::chat::links_tests::*` (hit regions at
1280x720, 800x480 and a narrow pane, with scroll; click per scheme with fakes; mismatch; no auto-open; selection vs click;
keyboard; menu; IDE routing; tool results). Limits: the story view and FLOWS show no links yet; the real BROWSER and IDE tiles are
checked through the request they receive; a user message broken by the wrapper in the middle of an address is cut there.

## Speed

| what | before | after |
|---|---|---|
| First request (system prompt + tool descriptions), tokens | 8,682 (78 tools) | about 3,500 (22 tools); 5,100 when media/radio are asked for |
| Tool groups | every tool, every time | web, files, pictures, vault and extras always; browser, media, radio, calendar and system when the message suggests them, or with `more_tools(group)`; a call to a tool of a hidden group loads it |
| Answer text | after the whole answer | shown as it is written (Ollama/OpenAI-compatible SSE, Anthropic SSE; Gemini answers in one piece) |
| Several independent read-only calls (`web_fetch`, `weather`, `stock_quote`, searches, reads, lists) | one after the other | together, up to 4 at a time, results kept in order |
| Ollama "thinking" | on (5.7 s for 17×23) | off for tool-using turns (`reasoning_effort: none`, 0.1 s); set `IYAKKI_THINK=1` to keep it |
| Model in memory | unloaded after 5 minutes | `keep_alive: 30m` on every request; loaded in the background at sign-in |
| Building the context (memory recall may call an embedding service) | on the UI thread | on the run's own thread |
| Tool result kept | 12,000 characters | 6,000 characters (about 1,500 tokens) |
| Latency | not recorded | `RunEvent::Timing` in every run record, `RunMeta` has model/tools/our ms, the chat footer says *first token in 1.2 s, total 4.8 s* |

HTTP connections to a provider are reused (one `ureq` agent per transport, kept by `Services`); request bodies are not compressed.

### Measured

`cargo run --release -p iyakki-agent --example assistant_bench` prints the tables below (all three parts, about 2 minutes).
`BENCH_TRACE=1` prints every event with its time, `BENCH_PROMPT=weather` runs one prompt, `BENCH_MODEL=qwen2.5-coder:7b` uses
another model of the same server, `IYAKKI_SETTINGS=/path/settings.json` points at another configuration. Settings are only read.

**Our own overhead** (scripted model with first token 400 ms and 15 ms per word, tools 350 ms; five fixed prompts):

| prompt | before: total | after: first text | after: total | our overhead | before the first model request |
|---|---:|---:|---:|---:|---:|
| what is the stock price of envx | 1827 ms | 1167 ms | 1832 ms | 1 ms | 1 ms |
| weather in chennai | 1828 ms | 1166 ms | 1833 ms | 0 ms | 0 ms |
| top stock market news (4 tools) | 2879 ms | 1166 ms | 1833 ms | 0 ms | 0 ms |
| play a tamil fm | 1827 ms | 1167 ms | 1833 ms | 1 ms | 1 ms |
| summarise this page | 1826 ms | 1166 ms | 1830 ms | 0 ms | 0 ms |

Targets: our overhead before the first model request under 30 ms (measured 0 to 1 ms, tool listing included for an administrator's
78 tools), a tool round trip under 20 ms excluding the tool (measured under 5 ms; the tests assert under 150 ms for a whole
two-round run on a loaded machine). Starting a run on the UI thread takes under 60 ms with a 120-message chat (asserted); before it
did the memory recall there.

**Real Ollama** (`nvidia_ollama` at 192.168.1.68, qwen3:8b, warm model, tool results canned so the web is not measured; one
run per cell, so treat differences below a second as noise):

| prompt | before: total | before: tokens | after: first text | after: total | after: tokens | tools used (after) |
|---|---:|---:|---:|---:|---:|---|
| what is the stock price of envx | 4253 ms | 19,219 | 1083 ms | 1569 ms | 7,701 | stock_quote |
| weather in chennai | 3299 ms | 19,072 | 1175 ms | 1420 ms | 7,651 | weather |
| top stock market news | 5645 ms | 19,750 | 1309 ms | 2379 ms | 12,879 | web_research ×2 |
| play a tamil fm | 4838 ms | 19,236 | 1443 ms | 1579 ms | 16,884 | radio_search, radio_play |
| summarise this page | 4512 ms | 19,181 | 1127 ms | 1477 ms | 7,629 | browser_page |

Where the saving comes from (first run of the same prompts with the thinking left on: 3.5 to 6.4 s, 7.9k to 18.7k tokens): about
60 % of the tokens go with the shorter prompt and the lazy groups; about 2.5 s a turn goes with the thinking switched off;
streaming makes the first words visible about 0.3 s before the end. "Tokens" is the provider's own count over the whole run
(prompt and answer of every round).

**Robustness, live.** A chat with three earlier refusals, then "what is the weather in chennai": qwen3:8b used the tool both
before and after (this particular history did not break it). The model that does break is `qwen2.5-coder:7b`, which prints its call
as text:

| | tools used | answer |
|---|---|---|
| before | none | `I cannot access live weather data.` (another sample: the raw JSON `{"name": "weather", ...}` as the answer) |
| after | weather | `In Chennai, today's weather is 31°C with a humid feel and a light breeze. There's a 40% chance of rain…` |

### What is still slow, and why

* **The model itself.** A first token takes about 1 s on this GPU server even with no thinking; a local model that is not in memory
  needs 5 to 60 s to load (the warm-up at sign-in and `keep_alive` avoid that, but only for Ollama).
* **Thinking models on hard questions.** With the thinking off a model reasons less. If answers get worse, start IYAKKI with
  `IYAKKI_THINK=1`. The plain chat (Agent off) leaves the model's default.
* **The web.** `web_research` opens several sites and follows links: 10 to 60 s is normal, and it is not made faster by anything
  here (calls to different tools in one turn do overlap).
* **MCP servers.** Their connections are made per run (`UserTools` is new for every question), so a configured server that is
  down costs up to 10 s before the first request. Not fixed.
* **Gemini** answers are not streamed.
* **Long prompts on small GPUs.** The tool descriptions are still 2,500 tokens of the first request. Cutting them further means
  rewriting each tool's wording; the groups already drop the rest.

## Thinking models

Reports from the real window: "crashes or does not show any response" with thinking models. What was found (qwen3:8b,
deepseek-r1:latest, deepseek-r1:1.5b, qwen2.5-coder:32b/7b on the Ollama server, in plain chat and Agent mode, real window under
Xvfb and recorded streams):

* **The live answer was never drawn.** The transcript is rebuilt only when a signature of its inputs changes, and the streamed
  answer was not part of it: while the model wrote, the chat kept showing the bare "Thinking" dot, and the text appeared only at
  the end. Fixed (the live text, the thinking and the seconds are in the signature); a UI test checks the text is drawn mid-stream.
* **Thinking was invisible and a thinking-only reply ended empty.** qwen3:8b with a tool offered (thinking on) was recorded
  thinking about the call and then ending the stream with no text and no tool call: the run finished with an empty answer, which
  the chat showed as nothing. Now: an empty answer is a `Failed` message in the chat ("The model returned an empty answer…"); a
  reply that is only thinking is first retried once with the thinking switched off, and if it still has no answer the thinking is
  shown as the answer with the note "The model wrote no answer, only its thinking".
* **Silence.** A model that never starts, or goes quiet for three minutes after its first word, now ends the run with a message
  instead of waiting for ever.

What the chat shows now, in plain chat and Agent mode:

* a **Thinking** block (collapsed, pulsing dot, click to open the thinking text) from the first moment; after two seconds it
  counts: *Thinking… 12 s*. The status line counts too. The block is replaced by the answer as soon as text arrives;
* the thinking comes from Ollama/OpenAI-style `reasoning`, `reasoning_content` or `thinking`, from `<think>…</think>` spans inside
  the content (also when a tag is split between two pieces), and from Anthropic `thinking_delta`s;
* it is **not saved and never sent back** to the model, and not in the run record; the footer says *thought 8.2 s, first token in
  9.0 s, total 9.5 s*, and the run record's `Timing` has `thinking_ms`.

The switch that turns thinking off for tool turns is `reasoning_effort: none` on Ollama's OpenAI-compatible endpoint, which the
server accepts for qwen3 (it stops thinking: 0.1 s instead of 5.7 s) and ignores for models that cannot toggle it (deepseek-r1
thinks anyway). If a server answers 400 "does not support thinking" the request is repeated at once without the switch and the
model is remembered, so the next one does not try. Plain chat keeps the model's own default (thinking on).

### Stall guard (a turn that ends with "let me try…")

A small model sometimes writes *"It seems there is an issue with the directory structure. Let me try a different approach…"* after a
failed tool and then ends its turn with no tool call; the person had to type "ok". In `assistrun.rs`, when a turn has text and no
call, `assistant_guard::detect_stall` decides whether it only announces an action ("let me", "I'll", "let's", "now I", "try
again", "a different approach") or is a short reply right after a failed tool, and does **not** fire when the text ends in a
question, asks the person for something, says it cannot or is blocked, or reports it is finished. Then the run keeps the prose,
shows a faint "continuing…" note and asks the model again with one request-only note (about 35 tokens, never saved or shown as
the person's message): *do it now with a tool call, or say plainly what blocks you*. Limits: at most 2 nudges per run; none after
a denial or an approval timeout; none when the last 3 tool results are the same error (the loop breaker makes the model explain
instead); none for a thinking-only reply. Tests: `assistrun_tests` (recovers into a tool call, genuine answer and question not
nudged, max 2, denial, repeated failures) and `assistant_guard::tests::stalls_are_told_from_answers_and_questions`.

### File errors the model can act on

`scope::explain_io` turns OS errors (not found, not a directory, permission, is a directory, disk full, read-only) into text that
names the part of the path at fault and the next step; `read_file`, `list_dir`, `write_file`, `run_command` (a working folder that
is gone) and `deploy_build` (dir) use it. `write_file` now creates missing folders above the file, after checking the write
scope on the final path (so a link out of the scope still cannot be used). A file in the middle of a path is named; if it is
empty it is said to be safe to delete. `write_file` refuses an empty file with no extension whose name looks like a folder
(`deploy`, `src`), which is what created the blocking `deploy` file in the reported loop. Tests: `scope::tests::explains_errors_and_creates_parents`,
`agent::tests::an_empty_file_where_a_folder_was_meant_is_named_and_not_repeated`.

### Planning on the board (IDE workspace chats)

`board_list`, `board_update` and `board_add` were already offered in workspace chats (they belong to the always-on `extra` group,
so lazy groups never hid them), but nothing told the model to use them and `board_add` failed when the workspace had no
`FEATURES.md`. The workspace prompt now says: for any job of more than one step, first `board_add` a planned row per step, then
work, then `board_update` to partial (with a comment) or done, and never done without a passed test or a verified check. The first
`board_add` creates the file (columns `ID | Feature | Status | Tests | Notes`). The Board tab shows a Create board button for a
workspace with no file. Test: `assistrun_tests::a_workspace_chat_plans_a_build_on_the_board_which_is_created_by_the_first_row`.

### Questions answered by a click

A person should never have to type "yes". When a message ends with a question, the chat shows answer buttons under it, above the
composer (stand-alone CHAT and the IDE chat pane).

* **Structured path.** The `ask_user` tool (Safe, no approval) takes `question` and `options[]` (up to 9; none means yes/no). The
  run ends its turn there: the final message is the question with the options numbered (so the transcript still reads right after
  a reload), and no further model call is made. The buttons use the exact option strings. Both prompts say: to ask the person
  something, call `ask_user` with short options instead of writing the question in prose.
* **Prose fallback** (`assistant_guard::detect_question`). The last sentence must end with "?" and contain a cue (would you like,
  shall I, do you want, should I, want me to, which, proceed, confirm, may I / can I, go ahead, prefer). Options come from a
  numbered or bulleted list just above or below the question, or from the "A or B" / "A, B or C" in the sentence; with none it
  is yes/no. Questions that open with what/where/when/how/who/why, statements, questions inside code, and "which" with no named
  options are not detected.
* **Buttons.** `Yes (Y)` `No (N)`, or `1 · A` `2 · B`, and `Other…` (hides the buttons and leaves the box for typing). A click
  sends the label as the next user message exactly as if typed (a half-typed draft is kept). With an empty box, 1-9 pick the nth
  button and Y / N answer a yes/no question; with text in the box keys type normally. The strip sits above the composer and the
  conversation ends above it, so nothing overlaps (tested at 1280x720, 700x520, 480x600 and a 300 to 340 px IDE pane); long
  labels are cut with an ellipsis. The transcript text stays selectable: the buttons are separate from it.
* **Lifetime.** They survive redraws; they go when any message is sent, on a new chat or an opened one, and are never saved.

Tests: `assistant_guard` (detection cases), `assistrun_tests::ask_user_ends_the_turn...`, and the chat's `ask_tests` (click, keys,
draft, list options, `ask_user` round trip, prose in the agent chat, geometry).

### Board tracker and times

`boardtrack.rs` moves the board rows **the assistant added** (remembered in `.iyakki/board-auto.json`) from what tools really did,
so a small model that never calls `board_update` still leaves a true board: planned to partial when `write_file`, `deploy_build`,
`deploy_load` or `k8s_apply` starts; done when it succeeds (a row naming two steps, such as build and deploy, needs both; a
verifying row needs a `deploy_status` whose workloads are all ready, or a passing test command); partial plus an `auto:` note with
the first line of the error when it fails. Rows a person wrote, and anything the model only says, never move a row. Each change is
audited as `board.auto` (the chat passes an audit closure with `ProjectTools::with_audit`). If the final answer claims progress
(done, deployed, running…) while such rows are still open, the run asks the model once, with a request-only note naming the rows,
to call `board_update` for each or say why. Status changes also write `Started` / `Finished` (UTC `YYYY-MM-DD HH:MM`); a board
without those columns gets them on the first timestamp, one pass, before `Notes`. Tests: `assistrun_tests` (Node.js sequence,
failed load, hand-written row untouched, one board nudge), `featureboard::tests` (transitions, rewrites, old boards, formatting).

### Auto mode: no asking permission

While approvals are automatic (Auto, or Autopilot) the system message of each request ends with *"Approvals are automatic: do not
ask whether to proceed, check, verify or continue; do it… Ask only when you are missing information or there is a real choice,
using ask_user."* (request-only, both prompts). If the answer still ends with a permission-style question about something the
assistant can do itself ("Would you like me to check if the service is responding…", "Shall I apply it?"), `guard::asks_permission`
sends it back once with *"Do it now; do not ask"* (it counts in the cap of two nudges). Questions that need information (which
port?), a real choice (npm or yarn?), or anything that always asks (delete, secrets, vault, browser) are left alone. In Ask mode
nothing changes. `deploy_status` now prints `reachable at:` lines (LoadBalancer external address, or node IP + NodePort) and says
a ClusterIP such as 10.43.x.x is cluster-internal, and both prompts say never to give a ClusterIP as a web address.

### Autopilot

`autopilot.rs` turns a goal into a series of ordinary runs (steps) that look like one run to the chat: step 1 plans on the board,
then each step carries one short message (*AUTOPILOT step 3: 2/7 rows done. Work on F-APP-03 "…"*; retries say *Attempt 2 of 3,
use a different approach*), so the whole board is never resent and the compact prompt stays enough. After each step the harness
reads only the rows the assistant added, and continues, blocks a row after 3 attempts (board note `autopilot: not finished after 3
attempts`), runs a final verification step, or stops. Budgets (never raised by the run): 40 steps, 30 minutes, the token budget
(400,000 if none is configured) and 3 steps in a row without any board change. Hard-ask calls still ask: the step waits on the
approval card (a note says *needs you*), and a refusal blocks the row. Pause (button, Ctrl+Shift+A, or typing a message), Stop,
sign-out and a read-only workspace end it; `.iyakki/autopilot.json` keeps the place for Resume. Audit rows: `autopilot.start`,
`autopilot.step`, `autopilot.pause`, `autopilot.stop`, plus `board.auto` for rows it blocks; finishing or stopping leaves an inbox
item. Tests: `assistrun_tests::autopilot_tests` and two chat tests.

### Done needs proof

`board_update` to `done` is refused by the harness unless the tracker has seen a verified success for that row (a tool result
that succeeded, for a deploy a ready rollout, a passing test command, the namespace seen to exist): *"refused: F-APP-01 cannot be
set to done: done needs a passed test or a verified check… Use partial or blocked and say why; the row is unchanged."* The
approval preview says the same. A Tests text or comment from the model never counts. The harness's own `board.auto` path is
unchanged, and it is the only way a row gets `verified` (kept in `.iyakki/board-auto.json`). A row blocked as unnecessary (for
example "Create namespace x" when the first apply creates it) can be finished by the tracker only when `k8s_status` shows the
namespace as `(exists)`; no other success revives a blocked row. Autopilot's summary and its counts use verified rows only
("2 of 3 rows done (verified), 1 marked done without a check (claimed, not counted)"). A repeated identical `board_update`
returns "already set… nothing was written" without touching the file. Tests: `assistrun_tests` (refusal, namespace, claimed rows,
duplicate update).

### Claims of done, pending rows, and the loop breaker

A refused `done` is no longer a dead end. The model gets the exact next action ("No verification has run for this row. Run
deploy_status now… or set it to `pending` with `depends_on`"), and the harness does the cheap check itself when a claim arrives:
`deploy_status` ready for deploy rows, the workspace's test command for rows about tests (`ProjectTools::with_test_command`). A
pass marks the row verified and done. A failure comes back once with its output and 3 attempts; then the harness sets the row
`pending` (when the notes name another row) or `blocked` and says so, so Autopilot moves on. The same refused claim twice changes
the row to `pending` instead of a third refusal. `pending` is a status with `depends_on` (the Depends column, or `Depends: …;`
before the notes) and its own lane; Autopilot skips pending rows, brings one back when the row it waits for is done, and brings
the rest back once each when nothing else is left; the summary lists the still-pending ones with what they wait for. A message
typed while Autopilot is paused is an interjection: it becomes part of the next step ("re-plan for this") and the saved goal stays.

### Thinking watchdog

A model call that only thinks (no visible text, no tool call) for 60 s, or sends no token at all for 120 s, is abandoned: the
stream is dropped at its next token (providers poll the callback on every thinking token), a note says *the model was thinking
for N s*, and the call is retried once with thinking off, the system message plus the last six messages, and a plain instruction
to answer directly. A second stall stops the run with a clear message; the partial thinking was already shown and stays in the run
record. `Settings.think_limit_secs` / `silence_limit_secs` change the limits (0 = default). Stop answers within a second and also
stops the helper's reading. Limits: a server that sends nothing at all keeps the HTTP read blocked until the client's own
timeout, although the run no longer waits for it. Tests: `assistrun_tests` (watchdog retry, second stall, Stop, defaults).

### Autopilot context, tokens and workspace history

**Why it ran out.** Each step used to resend the whole chat, and the budget counted every resent prompt, so a 16-step run spent
about 150k tokens a step on nothing new. Now each step's request is: the system message, `Goal: …`, and one step message with the
current row, the counts, the **running summary** and the few chunks **recalled** from the workspace history for that row. The
summary is written by the harness (done rows, blocked, pending, files changed, commands, last error, last answer), replaced at
every step and capped at 3,000 characters. Old tool output is not resent: it is a capped digest in the history, found again with
`history_search`. A scripted 16-step run: about 1.8k tokens per step at step 2 and step 16 (flat), against about 27k tokens with
the whole chat resent (`Context::compact = false`).

**Token budget.** Autopilot counts generated tokens plus the *new* part of each prompt (default budget 300,000), so it measures
work, not conversation length. A budget ending is a checkpoint: the chat gets *"Autopilot reached its token budget. Nothing is
lost… Resume (fresh context)"* with the summary, the state is saved, an inbox item is left, and Resume restarts with new budgets
(steps count from the resumed step) and a context rebuilt from the board and the summary. The step note says `~Nk tok last step`.

**Tools that repeat themselves.** `k8s_logs` of a pod a rollout replaced uses the current pod of the same deployment and says so;
an empty log says the container wrote nothing and what to check instead (`deploy_status`, the `reachable at:` address, events); long
logs keep the newest 6,000 characters. The loop-guard message names the alternative. When the guard stops a step, Autopilot does not
stop: the harness verifies the current row with `deploy_status` (done when everything is ready) or blocks it, and goes on.

**Workspace history (`history.rs`).** The in-house knowledge base holds one namespace per person and workspace
(`workspace__<person>__<workspace>.afkb`): answers, tool-result digests (500 characters), errors, decisions and summaries, each tagged
with kind, row, session and time. Credentials are redacted with `credguard` before indexing; the oldest tool, error and turn chunks
are pruned past 1,500 documents (decisions and summaries last); `forget_workspace` and `forget_session` remove it. Each step
recalls up to about 1.5k tokens for its row; a new session of the workspace starts from the latest summary; the Safe tool
`history_search(query, scope, kind, k)` reads it (scope `session` is this chat only, `workspace` every session). It can be turned
off with `Settings.no_workspace_history`. Limits: only Autopilot steps are recorded; the summaries are extractive; there is no screen
for the setting or the forgetting yet and deleting a chat does not yet forget its chunks. Tests: `assistrun_tests::autopilot_context_tests`,
`history::tests`, `k8stools::tests::logs_of_a_replaced_pod…`.

### Hard limits per Autopilot step

Guards that depend on the model's text can be beaten by a changing counter ("failed 78 times", "79 times"), so the limits are
counted by the harness: **3 board_update calls and 2 failed checks per row in one step**. The call after the limit is not run; the
harness sets the row to `pending` (a row it names as a dependency), `blocked` (the check failed twice) or `review` (work claimed, no
sensible check), ends the step with one line (*Row F-APP-02 is parked as blocked: …. Continue with the next row.*) and the next row
starts. A row whose check failed 3 times in the run stays parked until something changed (a file written, a command run, a row
verified). Board tool results are cut to 300 characters; the `deploy_status` dump is never appended. The repeat detectors compare
tool name, arguments and the head of the result with digits removed. The check fits the row: deployment rows use `deploy_status`;
code rows (or any row naming source files) need the named files, or the files written in this run, to exist, be non-empty and
parse (`node --check`, `python3 -m py_compile`, JSON), then the project's test command when set; test rows use the test command;
anything else goes to `review`, shown amber with a Mark done button on the card (the person's click counts as verified). With many
rows Autopilot adopts the board's rows when the goal is about them (feature, task, row, board, backlog), asks which ones when there
are more than 12 (all, planned only, one phase: chips), picks the next row whose dependency is done and shows
`step 5 · row 12 of 63 · … · ~N tok`. Before: one run called board_update 80 times in a step; now it runs at most twice.

### Fixing instead of advising, and an honest summary

*Failed checks.* A failed verification of a row ends the step (the model sees one short line) and the harness keeps the diagnosis: the
lines of the check output that say what is wrong (the `Diagnosis` and `Next` lines, pod reasons such as CreateContainerConfigError or
ImagePullBackOff). The next step's message carries it with the instruction to change the files or manifests and redeploy, not to describe
a fix. Only a failed check after something really changed since the last one (a file written, a command run, a build, load or apply)
counts as a fix attempt; three failed fixes park the row (pending with a dependency, else blocked), with the first diagnosis line in the
note. A failed check with nothing changed is a stall, and three stalls park it too.

*Advice instead of action.* In Autopilot, an answer that only proposes actions ("you should add USER 10001… run kubectl apply") with no
tool call in the step, while rows are open, gets the one nudge "do not describe: make the change now with a tool call". A step that still
changes nothing counts as a stall for its row.

*What the summary says.* "Commands run" and "Files changed" list only calls the harness executed: approved, with a result (a refused or
denied call, or a command printed in the model's text, is not listed). Commands the model merely wrote in its final answer are listed
apart as "Suggested, not run".

### Printed calls, duplicate rows, criteria

*Calls written as text.* A tool call the model prints instead of making is found even when it is pretty-printed over many lines, bare or in a
json fence, with prose before and after, in the `name`/`tool`/`function` + `arguments`/`parameters`/`args`/`input` shapes, and
`more_tools` is accepted although it is not a registry tool. It is executed and removed from the answer text.

*One row per title.* `board_add` compares titles without case, punctuation and spacing: the same title is not added twice; the reply says
`already exists as F-APP-01` and the new tests, files and acceptance criteria are merged into that row. Autopilot adopts existing rows
and so never adds a second row for a goal it already tracks.

*Tests are names, criteria are sentences.* A Tests entry counts as a test when it looks like a name or path (`crate::m::t`, `tests/a.py::test_b`,
`TestAdd`, `file.test.js`, a lowercase phrase of a few words); a sentence ("Backend deployment should be created successfully.") is an
acceptance criterion: listed as a checklist "CRITERIA (not tests)" in the row's detail, never counted in "n tests", never "not run".
`board_add`/`board_update` take `tests` (real names or paths) and `acceptance` (free text, an `Acceptance` column added when missing); a
sentence given as a test is filed as a criterion.

*After a failed check the diagnosis comes back in every following step* until the row is done or parked, so a step that only described
what to do does not lose it.

### It does the work itself

Both base prompts and every Autopilot step message state: you can read and write files and run tools; never ask the person to open, edit,
replace, create or save a file, to run a command or to "try the following command". `guard::asks_person_to_act` finds the opposite in an
answer (polite requests, "you need to … the file/Dockerfile/manifest/line/command", "run the following", "copy and paste", a tool call
printed as `deploy_build(dir=".")`), when the chat has file or command tools and the answer is not a question; the run sends it back once
with *"Do it yourself now with the tools; do not ask the person to edit or run anything"* (counted in the cap of two nudges, in Ask, Auto
and Autopilot). In Autopilot a step that still changes nothing is a stall for its row. A failed `deploy_build` returns the real failing
step and the Dockerfile's first 15 lines with numbers, and the next Autopilot step opens with the same block.

### Bugs with evidence

Failures are bugs, not features: `bug_report` (and `board_add kind: bug`) makes a `B-` row and captures the evidence of the failure in
`.iyakki/bugs/<id>/` (see docs/AI-IDE.md). The harness does it itself when a verification fails or `deploy_status` shows CrashLoopBackOff,
CreateContainerConfigError, ImagePullBackOff, ErrImagePull, OOMKilled…: once per signature (reasons + pod name pattern + image), the diagnosis names the logs folder, and a healthy
rollout resolves those bugs with the passing run kept. No tool success moves a bug and the model cannot set one done without a verified check. The dev skills and
the prompts tell the model to give bugs and failures their own `B-` row. Credentials, tokens and env values are redacted before writing and files are capped.

### Project state at the start of every run

`projectstate::build` reads the disk and writes the PROJECT STATE block (see docs/AI-IDE.md): stack and commands, the unfinished board rows with status, start
time, tests and notes (done rows only counted), open bugs, the last deploy facts the tools returned (`.iyakki/state.json`), test and coverage results, recently changed files,
the last session summary from the workspace history and the last Autopilot place. It is added to the system message of a plain run and to the message of every Autopilot step
(never from the model's memory: a changed `FEATURES.md` shows at once), capped at about 3,600 characters by cutting the middle and keeping the stack and the instruction.
The stack's language skill and the k3s practices are pinned in the context (`ContextOptions.pinned_skills`, at most two, before the skill chosen by similarity).
In Autopilot, a goal that is about the board's rows or says continue/resume adopts the rows that exist, and a run that finds owned rows on the board continues with the next
unfinished one instead of planning again; `board_add` of a title that exists adds nothing.

### No questions once the goal is done

The Autopilot final check no longer always ends with *"Tear down the dev environment?"*. The harness allows that single offer only after a deploy that succeeded
and was verified (no blocked row), while `k8s_status` still shows the namespace as existing, never for a teardown goal, and once per chat session (the session is
remembered in `.iyakki/state.json`). In every other case the final check says *summary only*: a closing question (would you like, please choose, a question mark) is sent back once,
the question is cut from what is shown, and `ask_user` is refused. A goal of *remove all / tear down / clean up / delete the environment* adds the row "Remove the namespace
and everything in it (tear down)", runs `k8s_teardown` (which still asks), counts its success or the namespace missing as the proof, and finishes with one summary. Prose that
repeats the question of an `ask_user` call in the same turn (the question or at least two of its options) is dropped, so the question is shown once, with its buttons.

### Autopilot with an external coding agent

The Autopilot harness is the same when the steps are carried out by Claude Code (`autopilot::Executor`, see
`docs/EXTERNAL-AGENTS.md`): only "run one step" changes. The step message (goal, PROJECT STATE, current row, last failure
diagnosis, running summary) is the agent's prompt with `--resume <session>`, so it keeps its own context; the harness keeps
everything that makes Autopilot reliable for small models and does not trust the agent's say-so:

* the plan is the board, rows are verified by kind (`boardtrack`): the agent's own `Write`/`Edit` and `Bash` calls reach the
  tracker through `Tools::note_external` once IYAKKI approved them and they succeeded, so a claim of done still runs the check;
* per-step caps: `--max-turns` from the run limits, wall time, and a tool that parks a row ends the step (`should_end_run`
  stops the agent's process); retries, no-progress and the step, time and token budgets are the harness's (tokens are the
  vendor's new input plus output, cache reads are not counted again);
* the cost cap: the vendor-reported cost of each step is added up and the run stops with "cost budget used" when the policy's
  cost budget is reached; the plan and progress are saved and Resume continues with fresh budgets;
* approvals: Auto answers for the agent only if the person allowed it in Settings → Agents, and `hard_ask` calls always ask;
  otherwise every action is a "needs you" card and the run waits;
* the summary lists only what ran: a file counts when its Write/Edit was approved and returned ok, a command when its Bash call
  was approved and ran; a refused call is not listed; commands the agent only suggested are "Suggested, not run";
* an answer that asks the person to do something the agent could do itself gets a note, and in Autopilot such a step changed
  nothing, so it counts as a stall (three in a row block the row).

### Parallel tasks: Autopilot several times at once

Each task of the Tasks tab (`docs/PARALLEL-TASKS.md`) runs its own Autopilot (`tasks::autopilot_runner`): its own `Context`
(root = the task's tree, its own board `.iyakki/tasks/<id>/FEATURES.md` holding only its rows, its own `autopilot.json`, session
`task-<id>`, budgets from the task), its own tools (`ProjectTools` rooted at the tree, so every file path and absolute path
argument outside it is refused, and a policy whose file scope is the tree), the same verification by row kind, honest summary and
`B-` bug evidence, and for Claude Code its own `--resume` session. Nothing a task does reaches the main workspace or the main
board: the manager reports a row `partial` when the task starts, `review` when it finishes and `done` only after the person
merges a task whose rows were all verified in the tree, under one board lock. The harness limits above apply per task, a task
over its budget (minutes, tokens, cost) is stopped and says which, approvals stay `hard_ask`-strict and are answered per task,
and the scheduler never runs more than the cap (default 3, hard 6). Tested with scripted models (the real Autopilot in a task
tree, the jail, two tasks together) and a fake `claude` script; never with a live model.

## How to benchmark and test

```
cargo run --release -p iyakki-agent --example assistant_bench            # tokens, sim, live
cargo run --release -p iyakki-agent --example assistant_bench sim        # only our overhead
BENCH_TRACE=1 BENCH_PROMPT=weather cargo run --release -p iyakki-agent --example assistant_bench live
cargo test -p iyakki-agent -p iyakki-sec
cargo test -p iyakki-shell --lib -- chat
```

Tests: `toolrepair` (loose JSON, names, argument coercion, printed calls), `assistant_guard` (denial table with true and false
positives, permission asks, condensing, loop counting), `toolblock` (reminder content and size, groups), `assistrun_tests`
(reminder placement, retry once, repair, text protocol, loops, timeouts, Stop, truncation, groups, parallel, streaming, timing,
`screen_capture`), `providers::tests` (SSE for OpenAI-style and Anthropic, fragments of tool calls, warm-up), the chat's
`assist_tests` (nothing of the machinery in the saved chat, the retry as a step, streaming and the footer, rolling summary,
`screen_capture` approval) and a shell test that the frame it draws is the one the tool receives.

## Limits to know

* Autopilot with an external agent: the cost total is kept in memory for the run (Resume starts it again from zero); Copilot and custom
  agents are advisors and Autopilot refuses them with the way out; no live vendor run was made.

* The phrase tables are English. A refusal in another language is not recognised by the guard (the reminder and the repair still
  work).
* A model that ignores both the reminder and the retry is shown as is, with the note about another model.
* The guard retries once per question. A second refusal, or a refusal after tools already ran, is not retried again.
* `screen_capture` shows everything on the screen, including a password field being typed in; that is why it asks every time.
