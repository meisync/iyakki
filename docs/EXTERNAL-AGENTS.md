# External coding agents (Claude Code, GitHub Copilot, custom commands)

IYAKKI can let a coding agent the person installed and signed into answer the chat, next to the built-in models (Ollama and
the others). IYAKKI only starts the vendor's own command-line program. It never handles a vendor password, never extracts or
forwards a login token, and an API key is only ever read from the vault and passed to the program as an environment variable.

Code: `crates/iyakki-agent/src/extagent/` (`mod.rs` the agent trait, settings and status; `claude_code.rs`, `copilot.rs`,
`generic.rs` the adapters; `run.rs` one run; `bridge.rs` the policy bridge; `proc.rs` the only place a CLI is started or
killed). UI: the model list in the CHAT and the IDE chat (Ctrl+P), `apps/chat_ext.rs`, and Settings → Agents
(`apps/settings_agents.rs`).

## What was verified, and how

Read on 2026-10-14 from the vendors' documentation. **No live run was made**: no vendor CLI is installed on the development
machine (`which claude copilot codex gemini aider` finds nothing; `gh` 2.46.0 is installed but has no `copilot` command, the
old `gh copilot` extension is not the current product), and the tests use fake CLIs (shell scripts) only. Anything marked
"unverified" below is code written against the documentation that has not met the real program.

### Claude Code (`claude`)

Sources: <https://code.claude.com/docs/en/cli-reference>, <https://code.claude.com/docs/en/headless>,
<https://code.claude.com/docs/en/agent-sdk/permissions>, <https://code.claude.com/docs/en/legal-and-compliance>.

| What | Verified |
|---|---|
| Headless | `claude -p "query"`; `--output-format text\|json\|stream-json`; `--verbose`; `--include-partial-messages` (needs `--print` and `stream-json`) |
| Sessions | `--resume <id\|name>`, `--session-id <uuid>`, `--continue`, `--fork-session`; the `result` message and the `system/init` message carry `session_id`; resuming reports the whole conversation's cost |
| Tools and permissions | `--allowedTools`, `--disallowedTools` (a bare name removes the tool; `Bash(prefix *)` denies only matching calls; `mcp__server__*` globs only after a literal server prefix), `--permission-mode default\|acceptEdits\|plan\|auto\|dontAsk\|bypassPermissions`, `--permission-prompt-tool <mcp tool>`, `--permission-prompts none` (v2.1.259+) |
| MCP | `--mcp-config <files or strings>`, `--strict-mcp-config`; with `-p` the CLI waits for pending servers up to `MCP_TIMEOUT` (30 s); `system/init` lists `mcp_servers` with a status and `mcp_server_errors` |
| Other flags | `--add-dir`, `--append-system-prompt`, `--model` (aliases `sonnet`, `opus`, `haiku`, `fable`, or a full name), `--max-turns`, `--setting-sources user,project,local`, `--max-budget-usd`, `--no-session-persistence`, `--version` |
| Sign-in | `claude auth login [--console] [--sso]`, `claude auth status` (JSON, exit 0 signed in, 1 not; `authMethod` is one of `none`, `claude.ai`, `oauth_token`, `api_key`, `api_key_helper`, `third_party`), `claude auth logout`, `claude setup-token`; `ANTHROPIC_API_KEY` in the environment; `--bare` ignores OAuth and the keychain |
| Events | `system/init` (`session_id`, model, tools, `mcp_servers`), `system/api_retry` (`attempt`, `max_retries`, `error`), `stream_event` with `event.delta` of type `text_delta`, `assistant` messages (`message.content` blocks), `user` messages carrying `tool_result`, a final `result` message with the final text, `session_id` and `total_cost_usd` |
| Permission answer | The SDK's `PermissionResult`: `{"behavior":"allow","updatedInput":{...}}` or `{"behavior":"deny","message":"..."}` |
| Process | SIGTERM exits with 143 and records no result; the CLI terminates the process tree of a running Bash command on SIGTERM |

Unverified (not stated in the pages read, or not run): the exact JSON that `--permission-prompt-tool` sends to the MCP tool
(`tool_name`, `input`, and whether `tool_use_id` is included; the bridge accepts it when present and otherwise matches the call
by tool name); whether MCP `tools/call` requests carry `_meta["claudecode/toolUseId"]` (used when present); the exact field names
of `usage` in the `result` message (`input_tokens`, `cache_creation_input_tokens`, `cache_read_input_tokens`, `output_tokens`,
the usual API names); that a prompt given as the last argument after `--` works with every flag above (the flags that take lists
are passed as `--flag=a,b` for that reason); that `--setting-sources project` leaves sign-in alone; that a request on standard
input works when it is over 60 000 bytes.

### GitHub Copilot CLI (`copilot`)

Sources: the live CLI (`copilot --help`, `copilot help permissions`), <https://docs.github.com/en/copilot/how-tos/use-copilot-agents/use-copilot-cli>,
<https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference>,
<https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference> (the last one only in part).

| What | Verified |
|---|---|
| Program | `copilot` (npm package `@github/copilot` per the install guide; the page read did not repeat the command) |
| Non-interactive | `-p PROMPT`; `-s/--silent`; `--output-format=text\|json` (JSONL); `--no-ask-user`; `--model=MODEL`; `--add-dir=DIR` (repeatable); `--share=PATH` |
| Permissions | `--allow-tool=TOOL`, `--deny-tool=TOOL` (repeatable or a quoted comma list, e.g. `'write, shell'`), `--allow-all-tools`, `--allow-all-paths`, `--allow-all-urls`, `--allow-all` / `--yolo`, `--available-tools`, `--excluded-tools`, `--allow-url`, `--deny-url` |
| Sign-in | `copilot login` (`--with-token` reads a token from standard input, `--host`, `--web-flow`, `--device-code`) or `/login` inside the program; token variables checked in order `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN` (a fine-grained token with the "Copilot Requests" permission, or an OAuth token; classic `ghp_` tokens are not supported) |
| Other | `COPILOT_MODEL`, `COPILOT_HOME`, `COPILOT_ALLOW_ALL`; `copilot --resume` / `--continue`; MCP servers in `mcp-config.json` under `~/.copilot` (`copilot mcp add --transport http NAME URL`) |

**Checked live on 2026-10-19** (Copilot CLI 1.0.98, musl build, on a real device; a stand-in MCP server written as a shell script with two
tools, run with a copy of the signed-in config in a scratch `COPILOT_HOME`; a handful of tiny prompts):

* `--additional-mcp-config=@FILE` (also an inline JSON string; never put a token in it) takes
  `{"mcpServers":{"iyakki":{"type":"stdio","command":..,"args":[..],"env":{..},"tools":["*"],"timeout":<ms>}}}`. `type` `stdio` is accepted;
  `timeout` is honoured (15 s made a 60 s call fail with "MCP error -32001: Request timed out"; without it a 75 s call finished).
  `copilot mcp list` has no such flag. `--disable-builtin-mcps` turns off `github-mcp-server` and `githubiq`.
* The model sees an MCP tool as `<server>-<tool>` (`iyakki-ping_ws`); the event carries `mcpServerName` and `mcpToolName`.
* Permission syntax (`copilot help permissions`): `--allow-tool` / `--deny-tool` take `shell(cmd:*)`, `write(path)`, `<server>(tool)`
  or just `<server>`, `url(..)`; denial beats allow, even `--allow-all-tools`. `--allow-tool=iyakki(ping_ws)` allowed that one tool
  and `--allow-tool=iyakki` the whole server.
* **There is no approval to answer in `-p` mode.** A tool that is not allowed is refused at once: `tool.execution_complete` with
  `success:false` and `error.code:"denied"` ("Permission to run this tool was denied due to the following rules: `shell`" for a deny
  rule, "Permission denied because no interactive user response was available" for a tool that is merely not allowed). The run
  does not wait and nothing is asked. So Copilot's own prompts cannot be sent to IYAKKI the way Claude's are.
* `--available-tools=view,glob,grep,iyakki` leaves the model exactly `view, glob, grep` and the server's tools; without it the model
  also has `bash`, `create`, `edit`, `web_fetch`, `task`, `sql`, `skill`, sub-agent and workflow tools. With `--deny-tool=write
  --deny-tool=shell` the file create and the shell command of a test prompt were refused and nothing was written.
* JSONL (`--output-format=json`): `assistant.message_delta` (`data.deltaContent`, ephemeral), `assistant.message` (`data.content`,
  `data.toolRequests`; content is empty in a turn that only calls tools), `tool.execution_start` (`toolCallId`, `toolName`,
  `arguments`, `mcpServerName`, `mcpToolName`), `tool.execution_complete` (`success`, `result.content` or `error.{message,code}`),
  `assistant.tool_call_delta` (progress of a call, not a call), many `session.*`, `model.*`, `assistant.turn_*` records, and a last
  line `{"type":"result","sessionId":..,"exitCode":0,"usage":{"premiumRequests":1,..}}`. No token counts and no answer text in `result`.
* Not seen: an error event (an unknown `type` containing `error` is read as a failure), `--resume` with a session id, an approval card
  that waits for minutes (the IYAKKI-side wait is bounded by the approval timeout; the call timeout is set to one hour).

**Copilot with IYAKKI's tools (Bridged, the default).** The one allowed server is IYAKKI's own, so the approval happens inside the
tool call: `--allow-tool=iyakki` lets the call through to the bridge, whose gate does what it does for Claude Code (refusal, risk,
`hard_ask`, grants, Auto only when allowed for this agent, an approval card, audit with the agent's name). Safety condition, which holds:
`--available-tools=view,glob,grep,iyakki` (the model has no write, shell or web tool of its own) plus `--deny-tool=write`,
`--deny-tool=shell`, `--disable-builtin-mcps`, `--no-ask-user`, never `--allow-all*`, the config file `0600` in the run's private
folder (`mcp-copilot.json`, next to Claude's `mcp.json`, removed with the run). A story run (no tools) offers only the server, whose
gate denies every call. Because Copilot's file tools are off, `write_file` (Ask risk, scope rules, 256 KiB limit) is offered to
Copilot only; commands go through `run_command`. Copilot's `view`, `grep` and `glob` still read inside the working directory and
`--add-dir` folders. Settings → Agents, key `v` on the Copilot row, switches to **advisor only** (the field `advisor_only`): no MCP, `--available-tools=view,glob,grep`, answer
shown, the person applies edits, a changed-files report if the workspace changed anyway.

### Copilot's handshake and the relay (found on a real device, 2026-10-19)

The first bridged Copilot runs stalled until killed ("MCP server connection failed … exited before completing the MCP initialize
handshake (terminated by signal 15)" in `~/.copilot/logs`). Cause, reproduced with the real relay and the real Gate: the relay copied
its standard input to the socket with `std::io::copy`, whose Linux fast path (splice) holds a message back until the next one arrives.
Copilot 1.0.98 sends `server/discover` first (id 0, a draft-spec method; any answer, even the -32601 error the Gate gives, lets it move
on; no answer makes it wait 10 s), then `initialize`, `notifications/initialized`, `tools/list` (recorded bytes are the fixture
`COPILOT_HANDSHAKE` in `extagent/tests.rs`). The first line stayed in the pipe, so no answer came. Claude Code never showed it because it
sends its next message at once. Fixes: `bridge::pump` (read, write, flush per chunk) in both directions; the Gate answers a line with
one write; the relay also finds its token in a `token` file (0600) in the run's private folder when the agent does not pass the `env`
block of the MCP config. Check on the device: the same probe with the new relay initialised in 9 ms instead of never.

## An IYAKKI app is a tile app (external agents)

`extagent::guidance` looks at the person's message with the built-in assistant's own routing (`target::resolve_intent`, plus "private
store" / "publish to the store" phrases). For a request for an IYAKKI app, tile, or app for IYAKKI (an MCP server or a skill for IYAKKI
get their own skill) the run's system text gets a paragraph saying a tile app is meant, published with IYAKKI's tools after the person's
approval, never a Python or Docker web service on the cluster, plus the bodies of `iyakki-app-author` and `iyakki-tile-author` (each cut at
6000 characters). Requests that name both IYAKKI and k3s or Docker are not guessed at (the person is asked, as for the built-in assistant),
and unrelated requests get nothing. Every bridged run's rules also say what IYAKKI is and name the tools. The bridge offers
`tile_scaffold`, `tile_test`, `tile_validate`, `tile_ship` (check, pack, sign with an automatically made key, publish: one approval card, no key or token asked of anyone), `tile_pack`, `tile_install_dev`, `app_*`, `publisher_keys`, `store_validate`,
`store_publish`, `store_server_status` and `store_search`; the Ask ones show an approval card, `store_install`, `store_update`,
`store_remove` and `store_rollback` are not offered.

## Mode chip and external agents

Autopilot or Auto on while the chosen agent may not use Auto (Settings → Agents → Allow Auto, key `a`) is honoured as far as IYAKKI can:
the harness plans, but every action still asks. The chip says so (`Autopilot · Claude Code: each action asks`, with the hint) and the
run's note reads "Autopilot is on, but Auto is off for Claude Code (not allowed in Settings → Agents)…" (this was the owner's "auto off").
Autopilot with an advisor-only agent shows `refused for <agent>`. The mode cell is live: a switch during a run releases a waiting card
and applies to the next call (`switching_the_mode_during_an_external_run…`).

### Custom command (aider, codex, gemini-cli, …)

Any program that takes the request as one argument (the word `{prompt}` in the command is replaced) or on standard input and
prints text. Everything printed is the answer. It also runs as an advisor, with the same changed-files report. These programs
were not looked at; the adapter is generic on purpose.

## Autopilot with Claude Code

Autopilot (the AI IDE chip) works with Claude Code selected. `autopilot::Context::executor` is the seam: it carries out one step
as a run. For an external agent each step's message (goal, PROJECT STATE, the current board row, the last failed check's
diagnosis, the running summary) is the prompt, sent with `--resume <session>` (the session of the chat, kept in
`ext-sessions.json`), with `--max-turns` from the run limits. Its events stream into the chat as usual. The harness, not the
agent, keeps: the board and the tracker, verification by row kind, retries, no-progress and step, time and token budgets
(vendor-reported tokens without cache reads), the cost cap (stop with "cost budget used" when the vendor-reported cost reaches
the policy's cost budget), pause, stop and take-over by typing, "needs you" cards, and the summary of what ran. Details and
the guard rules are in `docs/ASSISTANT-ROBUSTNESS.md`.

* The agent's own `Write`, `Edit` and `Bash` reach the board tracker through `Tools::note_external` after IYAKKI approved them
  and they returned ok, so rows about files and tests are verified as for the built-in assistant.
* Auto: only when the person allowed it for this agent (Settings → Agents); otherwise every action pauses the run on an
  approval card. `hard_ask` calls ask in both cases.
* A tool that ends the step (a parked row) stops the agent's process and its text is the step's answer.
* GitHub Copilot works with Autopilot (it is bridged by default). Copilot set to advisor only, and custom agents, are refused: "Autopilot
  needs an agent that works through IYAKKI's tools and approvals … switch Advisor only off" (Copilot) or "… Claude Code and GitHub Copilot
  can, <agent> runs read-only here" (custom), pointing to Ctrl+P (there is no one-click switch chip yet).
* The footer while it runs reads `autopilot · Claude Code · step 3 · row 2 of 7 · ~1.2k tokens · $0.12`.
* Unverified: how a real Claude Code behaves over many resumed steps (context growth, `--max-turns` hitting the cap, the
  permission-prompt input shape); the tests use a fake CLI that speaks MCP to the real bridge (`python3` needed; the tests
  skip without it).

## Installing from the CLI tile

The CLI tile's catalogue (kubectl, helm, terraform, …) also lists **Claude Code** (`claude-code`), **GitHub Copilot CLI**
(`github-copilot`), **OpenAI Codex CLI** (`codex`), **Gemini CLI** (`gemini-cli`) and **Aider** (`aider`), with Install, Update
(install again), Remove, and for the first two Login, Logout (Claude Code) and Status buttons. Settings → Agents uses the same entries
(its Install key opens the same confirmation), so there is one place that knows what is installed.

Verified install commands (read 2026-10-14):

| Tool | Command used | Source | Requirement |
|---|---|---|---|
| Claude Code | The native build: `downloads.claude.ai/claude-code-releases/latest` names the version, `<version>/manifest.json` the SHA-256 per platform (`linux-x64`, `linux-arm64`, and `-musl` for musl systems), `<version>/<platform>/claude` is the program. This is what Anthropic's `install.sh` does (read 2026-10-18, not run); the tile does it in Rust and skips the script's `claude install` step. Fallback only when that download cannot be fetched and a Node.js is present: `npm install -g @anthropic-ai/claude-code` | <https://code.claude.com/docs/en/setup> ("Alpine Linux and musl-based distributions": `libgcc`, `libstdc++` and `ripgrep` at runtime and `USE_BUILTIN_RIPGREP=0`; the musl file itself links only musl libc) | None for the native build; a Pro, Max, Team, Enterprise or Console account |
| GitHub Copilot CLI | The standalone build `copilot-linux[musl]-<x64\|arm64>.tar.gz` from `github.com/github/copilot-cli/releases/latest`, checked against the release's `SHA256SUMS.txt` (what GitHub's `gh.io/copilot-install` script does, read 2026-10-18, not run; GitHub also publishes a musl build, which links only musl libc) | <https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli> | None; a Copilot subscription |
| Codex CLI | `npm install -g @openai/codex` | <https://github.com/openai/codex> | Node requirement not stated on the page |
| Gemini CLI | `npm install -g @google/gemini-cli` | <https://github.com/google-gemini/gemini-cli> | Node requirement not stated on the page |
| Aider | `pip install aider-chat` into its own environment (the Azure CLI mechanism) | <https://aider.chat/docs/install.html> (Python 3.9 to 3.12 for pip) | `python3` with `venv` |

How the npm install runs: IYAKKI shows the exact command and the folder, asks, and only then starts `npm install -g <package>` as an
argument array (no shell, never `sudo`) with `npm_config_prefix` set to a folder under the CLI tile's own folder (`cli/pkgs/<id>/latest`)
and `npm_config_cache` under its `tmp` folder, **for that one child process only**: IYAKKI's own environment, `~/.npmrc` and
`~/.bashrc` are not touched. The program is linked into the tile's `bin` folder, which the chat's `run_command`, the CLI terminal
and the external-agent code (when no command is set in Settings → Agents) look in first. The IDE's workspace terminal does not
search that folder yet. Claude Code and Copilot need no Node.js. For the npm tools (Codex, Gemini CLI), without Node.js 22 or newer (and npm) the catalogue's own **Node.js** entry (the active LTS line, 24 in October 2026) is installed first, in the same job and
the same data folder (`cli/pkgs/nodejs`; the newest release from nodejs.org, checked against `SHASUMS256.txt`; on a musl system such as the
IYAKKI images the musl build from unofficial-builds.nodejs.org, plus `libstdc++.so.6` from Alpine's package repository when the machine has
none, with `bin/node` becoming a small launcher that sets `LD_LIBRARY_PATH`). The confirmation says so before anything runs; if that step
fails, the install stops with "Node.js is needed first for ... and could not be installed: <why>". The same holds for Python tools
(Lemonade, Aider, esptool, Azure CLI): without a Python 3 that has `venv` and pip, the catalogue's **uv** and **Python 3** (3.12 from
python-build-standalone, downloaded by uv into `cli/pkgs/python`) are installed first. After the
install finishes, `<program> --version` is read, the external-agent detection runs again at once (Settings → Agents and the model
list update without a restart). **Login** (and **Logout**) are described in the next section.

Not offered, on purpose: Anthropic's native installer `curl -fsSL https://claude.ai/install.sh | bash` (verified on the same
setup page) and GitHub's `curl -fsSL https://gh.io/copilot-install | bash`. They pipe a script to a shell, install outside IYAKKI's
data folder (`~/.local/bin`, `~/.local/share/claude`), and the Claude one updates itself in the background, which would break
"nothing outside the data folder is written". If you want them, they need their own extra confirmation and a separate remove path.

## Login and Logout buttons

Once Claude Code or GitHub Copilot is installed, a **Login** button and a status chip sit next to it in four places: the CLI
list (detail pane), Settings → Agents (keys `l` and `o`, and the buttons), Quick install (IYAKKI menu, F10) and the chat's model
list (Ctrl+P), where an agent that is installed but not signed in says **Sign in first** with the same Login button. The chip
reads "signed in as <account>", "not signed in" or, when the program cannot say, why. Nothing is asked on the interface thread:
the chip comes from the cached status (`extagent::StatusCache`, refreshed after an install, after a login or logout, and by
Detect). Without an installed tool there is no button.

**Login** opens a dialog over the screen and runs the vendor's own command in a pseudo-terminal (`extagent/login.rs`), so it
behaves as it does in a terminal, in the person's own home folder (`HOME` and, for Claude Code, `CLAUDE_CONFIG_DIR=<home>/.claude`;
the home folder is `/home/<user>` on an image, which is a bind mount of `<data>/home`, so the login survives a reboot). The dialog
reads what the program prints (colour codes removed, addresses wrapped by the program joined again) and shows:

* **Claude Code** first asks how to sign in: *Claude subscription* (`claude auth login --claudeai`, the default), *Anthropic
  Console, API billing* (`--console`) or *SSO* (`--sso`). The program prints a sign-in address and waits at
  "Paste code here if prompted"; the dialog shows the address (copyable, wrapped, with a QR code to scan with a phone) and a field
  for the code the browser shows, which is forwarded to the program. Source: <https://code.claude.com/docs/en/authentication>
  ("If your browser shows a login code instead of redirecting back after you sign in, paste it into the terminal at the Paste code
  here if prompted prompt"; the same page says credentials are in `~/.claude/.credentials.json`, or under `CLAUDE_CONFIG_DIR`).
* **GitHub Copilot** runs `copilot login --device-code`: the program prints a one-time code (`ABCD-1234`) and
  <https://github.com/login/device> and then polls, so the dialog shows the code large and copyable and the address with a QR
  code, and needs no field. Source: <https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli>
  (device flow is the default on a machine without a browser; with no keychain the program asks whether to keep the token in plain
  text in `~/.copilot/config.json`: that question is shown with the same field, answer `y` or `n`).
* A live status line, **Cancel** (the program and everything it started are stopped; leaving the dialog or 15 minutes do the
  same), "Signed in" on success (the chip refreshes), and on failure the program's own last lines in plain words with addresses and
  token-like words cut out, and Try again.

**Pasting the code.** Claude's page shows a long code (`code#state`, a hundred characters or more). The dialog gives three ways
to get it in without typing:

* **Open in browser** opens the sign-in address in a new tab of the BROWSER tile (the full engine when the image has it) and hides
  the dialog; the login program keeps waiting. Sign in, copy the code on Claude's page, leave the browser (Back / Esc): the dialog
  returns by itself, and if the clipboard holds something that looks like the code a button **Paste the copied code (N characters)**
  offers it. Nothing is filled in before a tap. If the program ended while the browser was open (it waits 15 minutes), the dialog
  says so with **Try again**. (The tab is opened with `open_item("newtab:<address>")` of `BrowserApp`; the copy from the page to
  IYAKKI's own clipboard is the browser's part.)
* **The phone relay (second QR).** While a login waits for a code, the dialog shows a second QR code and a short address,
  `https://<device address>:<port>/login/<token>` (`extagent/relay.rs`, `iyakki-remote/src/relay.rs`). On the phone that signed in,
  open it, paste the code in the one big box and press Send; the device forwards it exactly as if typed. The page is served on the web
  front end's own port when it runs, else by a listener opened for the length of the login. The token is 128 random bits, works
  for one submission (the dialog re-arms it if the vendor refuses the code), and dies with the dialog or after 15 minutes; only a text
  body up to 8 KiB is read, 20 requests a minute per address, only private and link-local addresses are answered, and with no login
  waiting every path gives the same "not valid" page. Only "login relay used" is logged; the code never is. The certificate is the
  device's own, so the phone's browser warns once. Copilot's device-code flow needs no code to be pasted, so it has no second step.
* **Paste / typing with a check.** A **Paste** button reads the clipboard. Whatever is in the field is cleaned (surrounding spaces,
  line breaks and quotes; a code wrapped over lines is joined; a whole redirect address `...?code=X&state=Y` becomes `X#Y`) and checked
  before sending, in large monospace type: its length, "has spaces", "has a character no code has", "looks too short", "Claude's code
  usually has a # in the middle". Problems block the send, doubts do not. If the vendor program refuses the code and asks again, the
  dialog stays open with the program's words and the field ready for another try.

**Logout** runs `claude auth logout` with the same home folder (Claude Code); for Copilot see the next section.

**GitHub Copilot's token and the vault.** On a machine with no keychain Copilot asks to keep its token in plain text in
`~/.copilot/config.json` (`authTokens`, with `loggedInUsers`; seen on a real device). Right after a successful login IYAKKI moves it: the
token and the account go into the person's own vault as the entry `github-copilot` (a small JSON object), the token is taken out of
the file (everything else stays: the comment header, the other settings, the account list; the file is rewritten with mode 0600), and
the audit records only "agent.login.vault imported". Whenever IYAKKI starts Copilot for that person (an AI IDE or chat run, the CLI
terminal) the token is given to that one process as `COPILOT_GITHUB_TOKEN`, which the program reads before `GH_TOKEN`, `GITHUB_TOKEN` and
its stored token (the vendor page named above); it is never put in IYAKKI's own environment, on a command line, in a log, or given to
another program or person. The chip then reads "signed in as <account> (token in your vault)". **Logout** deletes the vault entry,
any token still in the file and the remembered account (Copilot has no logout command outside the program). With the vault locked
the dialog says so and leaves the plain file alone ("unlock your vault in Settings → Vault, then sign in again"); the chip then says the
sign-in cannot be checked rather than "not signed in".

**Claude Code and the vault.** Claude can read a login from the environment (`CLAUDE_CODE_OAUTH_TOKEN`, made by `claude setup-token`,
<https://code.claude.com/docs/en/authentication>), but that token is for a subscription only, "can only make model requests" (no Remote
Control, no claude.ai connectors), is printed once instead of being stored, and is a different sign-in from `claude auth login`. So
Claude's login stays where Claude keeps it, `<home>/.claude` (`CLAUDE_CONFIG_DIR`), on the data volume in the person's home; it is not
moved into the vault.

**One home for everything.** The login, the logout, the status lookups behind the chip, the model list's "Sign in first", the agents'
runs, the CLI terminal and the Copilot token all use the same home: `/home/<user>` on an image (`extagent::login::login_home`, which
`Services::agents_cfg` hands to every status lookup and agent run as `HOME`), never the inherited `HOME` of the IYAKKI process (which is
`/` on the images). The check that reads a program's version after an install runs in a throw-away home, so a program that writes
a settings file at its first start cannot leave one in the person's home or in `/`.

What IYAKKI keeps: nothing. The program's output is held in memory while the dialog is open; the token or code a person types goes
to the program only; the audit gets "agent.login.start", "agent.login.finish <agent> ok|failed|cancelled" and "agent.logout", never an
address, a code or a token. The credentials stay where the vendor program keeps them (in the home folder), as before.

**Status of Copilot.** GitHub documents no command that reports whether Copilot is signed in (only `/user` inside the program), so
IYAKKI reads the program's own settings file (`$COPILOT_HOME` or `~/.copilot`, `config.json`: `last_logged_in_user` /
`logged_in_users` with a `login`; the file starts with `//` comment lines) and the token variables the program checks
(`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`). The shape of that file was not verified against a live CLI: when it is not
recognised the chip says the sign-in cannot be checked, never "not signed in".

**Not verified (nothing was tried with a real account):** the exact words of the programs' prompts (a code to paste, a yes/no
question) are recognised by pattern and may differ; the `--claudeai` flag is taken from the owner's brief and the `claude auth
login --help` of current builds; `claude auth status` naming the account in an `email` field is read leniently (the method alone is
shown when it is absent).

Pictures: `docs/media/shots/login-*.png` (made by `cargo run -p iyakki-shell --example login_shots` with stand-in programs).

## Models

Every agent expands into its selectable models, in the model list of the CHAT and the IDE chat (Ctrl+P: the agent line, then its
models indented under it, then "Custom model id…") and in Settings → Agents (`m`). Where the lines come from, in this order:

1. **The CLI's own list**, only if the CLI can print one without a session. **Verified: neither can.** Claude Code documents no
   such command (its `/model` picker and `/status` are interactive; <https://code.claude.com/docs/en/model-config>), and the
   Copilot CLI documents `/model` (alias `/models`) as an interactive picker only
   (<https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference>); no vendor CLI was installed to
   check `--help`. So nothing is run for them. The mechanism exists for custom agents: a "Models command" (one model per line,
   an optional note after a tab, two spaces or ` - `; a note saying unavailable / not available / requires marks the model "not
   available on your plan", nothing else is ever assumed). It runs only after the person pressed Detect, with a 10 s limit, and the
   answer is kept 24 h in `ext-models.json` (the agents cache).
2. **The documented list, shipped as a fallback**, labelled "built-in list" and dated (`extagent::BUILTIN_VERIFIED`,
   2026-10-16). Claude Code, from the model-config page: `default` (Default: let the CLI choose; the account's own model),
   `sonnet` (daily coding), `opus` (complex reasoning), `haiku` (fast, cheap), `fable` (hardest, longest tasks; may need usage
   credits), `opusplan` (Opus plans, Sonnet carries it out). The page also names `best`, `sonnet[1m]` and `opus[1m]` and full ids
   such as `claude-opus-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`, `claude-sonnet-5`; those can be typed as a custom id. Copilot:
   its documentation names no ids for `--model`, so only Default is shipped.
3. **A custom model id** the person types (letters, digits and `. _ - : / @ + [ ]`, at most 80 characters, never a space, quote,
   `$`, `;`, `&`, `|`, `<`, `>` or a leading dash: it ends up as one argument of the command line). It is remembered for that agent
   (`custom_models`) and shows as a line next time.

The choice is per chat and remembered per workspace in `.iyakki/agent` (`claude-code opus`; Default is just `claude-code`), or as the
agent's own setting (Settings → Agents) outside a workspace. It is passed as `--model <id>` (Claude Code) or `--model=<id>`
(Copilot); Default passes nothing, even if the agent's setting names a model. The footer reads
`agent: Claude Code · Sonnet · session abcd1234`. Lines are marked "not available on your plan" only when the CLI's own list said
so; the built-in list never claims anything about a plan.

## How a run works

1. The chat builds the same context as for the built-in assistant (the PROJECT STATE of the workspace, skills, memory, the
   feature board rules with `B-` and `F-` ids) on the run's own thread and passes it as `--append-system-prompt` (Claude Code) or
   in front of the request (others), with the rules: do it yourself, use the board tools, call `history_search` first, stay in
   the workspace, how the cluster tools replace raw `kubectl`.
2. The program starts in the workspace folder (or the chat's own work folder) with no shell, an array of arguments, its own
   process group, and only `PATH`, `HOME`, `LANG`, the XDG folders and its own config variables from the person's environment
   (so the person's other tokens are not inherited). An API key from the vault is added as one variable (`ANTHROPIC_API_KEY`,
   `COPILOT_GITHUB_TOKEN`, or the variable named for a custom agent); it is never in the arguments, the events or the audit.
3. Its event stream becomes the chat's events: partial text → streaming answer, thinking blocks, `tool_use` + `tool_result` →
   tool blocks (IYAKKI's own tools are shown under their own names), `result` → the final answer, the vendor's token numbers in
   USAGE and the "last run" footer, cost in cents. The vendor's session id is stored per chat and agent and used with `--resume`.
4. Stop (or the time limit, or 5 minutes without any output) sends SIGTERM and then SIGKILL to the whole process group, reaps
   it and the leftovers (a dev server the agent started does not outlive the run).
5. Failures are plain: not installed (the exact install command), not signed in (the vendor's sign-in command), rate limited,
   network down, an error `result`, a non-zero exit (stderr is kept in the run log), an empty answer. Lines that are not JSON
   become notes.

## The policy bridge

Claude Code is started with `--mcp-config` (a file in a private 0700 folder) and `--strict-mcp-config`, so it sees one MCP
server, `iyakki`: this program again, `iyakki --mcp-tools <socket>`, a relay between its standard input and output and a Unix
socket the run listens on (the first line is a per-run token passed in the environment). Behind the socket is the same `Tools`
of the chat, so the same refusals apply.

* Offered tools: `board_*`, `bug_report`, `deploy_*`, `k8s_*`, `history_search`, `ide_browser_*`, `data_*`, `registry_*`,
  `run_command`, `job_*` (detached jobs: `job_start`, `job_wait`, … see [`JOBS.md`](JOBS.md); the agent is told to use `job_start` for anything over
  about 30 seconds), and the read-only (Safe) status tools. Never offered: `vault_request`, `screen_capture`, `ask_user`,
  `write_file`, `browser_*`, outside MCP tools.
* A call: policy refusal (production read-only, devcluster guard, namespace confinement…) is the tool's answer and nobody is
  asked; Safe tools run; a pre-approved grant runs; Auto runs what `agent::hard_ask` does not name, **only if the person allowed
  Auto for that agent** (Settings → Agents, off by default); otherwise an approval card appears in the chat (and the INBOX as
  for any run) and the call waits. Teardown, deletes, vault and outside-the-workspace commands always ask even in Auto.
  "Allow for this session" on that card is kept for the whole chat, shared with the chat's own runs and Autopilot steps, and
  covers the same tool in the same scope only (a plain `Bash` command is covered by its program and subcommand, a pipeline or
  redirect only as the exact line, a file edit by "a file in this workspace"); calls that always ask are never covered.
* `--permission-prompt-tool mcp__iyakki__approve` sends Claude Code's own prompts (Edit, Write, Bash, WebFetch, …) to the same
  place: edits are allowed only inside the workspace (symbolic links resolved) and never in `.iyakki/` (use the board tools) or
  `.git/`; reads outside the workspace are refused; `Bash` with kubectl, k3s, kubeadm, minikube, kind, helm, sudo, su or docker
  and podman remove/stop/system actions is refused with the name of the IYAKKI tool to use (also as `--disallowedTools` rules);
  other commands ask (destructive or outside-the-workspace ones even in Auto); unknown tools of the agent always ask.
* `--allowedTools=mcp__iyakki__*` so the bridged tools are not asked twice (IYAKKI asks, once). `--permission-mode default` and
  `--setting-sources project` keep the person's global allow rules out; a project's own `.claude/settings.json` inside the
  workspace can still add allow rules.
* Every decision is written to the audit log with the agent's name (`extagent.start`, `extagent.tool`, `extagent.decision`,
  `extagent.refused`).
* **The approval mode is live.** Claude Code starts with `--permission-mode default` and every prompt comes to the bridge, so
  the vendor's own mode never needs to change: the `Gate` reads the chat's live mode (`RunLimits::live`) for every permission
  prompt and every IYAKKI tool call, and while a card waits. A switch to Auto-approve safe applies to the next prompt and
  approves a waiting card that Auto would run (`approved by mode-switch`); Ask first makes the next prompt ask. Auto still needs
  "Allow Auto" for that agent (Settings → Agents): without it the switch changes nothing and the chat says so once. Always-ask
  prompts ask in every mode. Autopilot switched on or off applies at the next turn or step boundary, as for the built-in
  assistant. Limits: a prompt the CLI already answered is not taken back; custom agents run as advisors with no
  tools, so the mode has nothing to decide for them.

## Limits to know

* Autopilot can drive Claude Code and GitHub Copilot (bridged); it refuses custom agents and Copilot set to advisor only, and says why.
* Custom agents (and Copilot when set to advisor only) are advisors, and IYAKKI cannot stop them writing; it reports it afterwards.
* The vendor's sign-in is interactive (a browser or a code): the Login button runs it in a pseudo-terminal and shows its address,
  code and QR code (see "Login and Logout buttons"); a sign-in method the dialog does not know is done in a terminal on the computer.
* The vendor's cost figures are client-side estimates and can differ from the bill; IYAKKI's token/cost budgets are not enforced
  on a vendor run, only recorded.
* A chat is answered by one backend at a time; the built-in model's history and the vendor's session are separate, so the
  context an agent has of earlier messages is its own session.

## Terms of service (decision for the owner)

IYAKKI drives the unmodified CLI the person installed and signed into and does not touch their credentials. Points to check
before this ships to other people:

* **Anthropic**: <https://code.claude.com/docs/en/legal-and-compliance>. Subscription OAuth "is intended exclusively for
  purchasers of … subscription plans and is designed to support ordinary use of Claude Code"; developers building products on
  the Agent SDK "should use API key authentication"; third parties may not "route requests through Free, Pro, or Max plan
  credentials on behalf of their users" or "collect, store, or intermediate Claude.ai credentials or session tokens". The same page
  says an end user may sign in "to the unmodified Claude Code binary with their own Claude subscription, including where a
  platform hosts Claude Code". IYAKKI as designed (the person signs in with the vendor's own command, the binary is not
  modified, no token passes through IYAKKI, an API key only comes from the person's own vault) fits that text, but it is a legal
  reading, not advice; "preinstalling or running Claude Code in your products" requires the Commercial Terms, so do not bundle
  the binary in the image without agreement. Advertised plan limits "assume ordinary, individual usage"; unattended or
  Autopilot-style heavy use on a subscription may exceed it. Do not use the names or logos in IYAKKI branding (the UI says
  "Claude Code (agent)" in plain text only). Terms: <https://www.anthropic.com/legal/consumer-terms>,
  <https://www.anthropic.com/legal/commercial-terms>, <https://www.anthropic.com/legal/aup>.
* **GitHub**: Copilot CLI use is subject to the GitHub Copilot terms
  (<https://docs.github.com/en/site-policy/github-terms/github-terms-for-additional-products-and-features#github-copilot>) and the
  Acceptable Use Policies (<https://docs.github.com/en/site-policy/acceptable-use-policies/github-acceptable-use-policies>).
  Tokens must be the person's own (fine-grained with "Copilot Requests"); do not share one account across people. These pages
  were not read for this document.
* **Custom commands**: the person's own responsibility; IYAKKI starts what they typed.

## Project context for external agents

The PROJECT STATE an external agent starts with begins with the workspace's **PROJECT CONTEXT** (`.iyakki/context.json`, the Context tab of the AI
IDE): shared by the whole team and every agent, committed to git. The person's and trusted entries are instructions, unreviewed entries
written by agents are fenced and labelled as data. The agent also gets `context_list`, `context_get`, `context_add`, `context_update`,
`context_archive` (aliases of the `note_*` tools) through the policy bridge; an entry it adds carries its label (for example `Claude Code`) as
author, whatever the model puts in the arguments. When the person says "remember that ...", "forget ...", "pin that" in the chat the harness
changes the context itself before the agent is started. With the Context tab switch **Share with external agents via CLAUDE.md / AGENTS.md** on
(also in Settings → Agents), the same entries are kept between `<!-- iyakki:context:begin -->` and `<!-- iyakki:context:end -->` in the
workspace's `CLAUDE.md` / `AGENTS.md` (blocks with the older `iyakki:notes` markers are taken over), so the agent also reads them when started outside
IYAKKI; nothing outside the markers is touched. See `docs/AI-IDE.md` (Context).
