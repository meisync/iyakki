# FLOWS: a workflow IDE with an agent that watches the runs

FLOWS (Work group, icon: three connected nodes) is where you **design, run, schedule and watch workflows**: graphs of steps (HTTP
calls, commands, browser steps, gRPC calls, agent steps, approvals...) that run on a timer or on demand, retry the way you say,
and, when one fails for good, get looked at by the assistant. The assistant can fix a *simple* problem itself, or put a card in
front of you ("needs you") with a proposed change to approve. Flows are files, versioned and published like code, and running
tasks can be told to pick up a fixed step.

Name: **FLOWS** (short, fits the tile, says what it is). The graph is a *flow*, one execution is a *run*, a box on the canvas is a
*node*.

```
 screen (apps/flows.rs, flows_canvas.rs, flows_run.rs, flows_board.rs)       assistant tools (agent/flows/tools.rs)
        |  polls snapshots; commands go over a channel                              |  same Env, same engine
        v                                                                           v
   iyakki_agent::flows   glue: Host (http, cli, browser, agent, notify, vault), runners (local, k3s Job), tools, Env
        |
        v
   iyakki_flow  (crates/iyakki-flow: no UI, no network, no process; serde types)
        model -> expr -> retry -> kinds (NodeKind registry) -> engine (DAG scheduler, runs, persistence)
        sched (cron, intervals, missed, overlap)   store (versions, publish, diff, library)   heal (patch, classify, budget)
        Host trait: everything that touches the world; FakeHost for tests
```

The engine crate has **no dependency on the agent, the shell or any network or process code**. Everything it needs from outside
is behind the [`Host`] trait (one method per capability, each with a "not available here" default). That is the seam a remote
backend uses later (`docs/EMBEDDING-AND-BACKEND.md`): `Flow`, `Run`, `Patch`, `Command` and `Snapshot` are all `serde`, so a
remote engine only has to carry those types.

## Data model (`iyakki-flow/src/model.rs`)

| Type | Fields |
|---|---|
| `Flow` | `id`, `name`, `description`, `nodes`, `edges`, `inputs` / `outputs` (name, type, required, default), `triggers`, `defaults` (`timeout_ms`, `retry`, `heal`, `runner`, `max_parallel`, `max_concurrent_runs`, `keep_runs`), `vars`, `meta` (`version`, `label`, `author`, `created_ms`, `changelog`, `modified_by`) |
| `Node` | `id`, `kind` (`start`, `end`, `http`, `cli`, `browser`, `agent`, `script`, `condition`, `switch`, `parallel`, `join`, `delay`, `approval`, `notify`, `subflow`, `foreach`, `remote`, `grpc`, `thrift`), `name`, `config` (JSON, shape per kind), `retry`, `timeout_ms`, `runner` (`local`, `k3s-job`, `remote`), `heal`, `track` (`pinned`, `latest-published`, `compatible`), `lib` (a reference into the node library), `pos` (canvas x, y), `on_error` (`fail`, `continue`) |
| `Edge` | `from`, `port` (the output port: `out`, `true`, `false`, a case name, `default`, `error`), `to`, `when` (an optional expression: the edge is taken only if it is true) |
| `Run` | `id`, `flow_id`, `flow` (the pinned snapshot, with the version number), `trigger`, `status`, `inputs`, `outputs`, `nodes` (a `NodeRun` per node: state, attempts, inputs, outputs, error, started and ended times), `heal` (cards), `patches`, `idempotency_key`, `parent` (for sub-flows), `log` (capped) |

Run status: `scheduled` (will start at a time), `queued` (waiting for a free slot), `running`, `blocked` (needs you: an approval or a
heal card waits), `retrying` (a node is between attempts), `healing` (the agent is looking), `failed`, `succeeded`, `cancelled`,
`interrupted` is a node state, not a run status (see crash recovery). The board's lanes group them: Scheduled, Running (running,
retrying, healing, queued), Blocked / needs you, Failed, Done (succeeded, cancelled).

Flows and nodes are **JSON files**. The text view in the designer shows and edits the same JSON, pretty printed (YAML was
considered: there is no YAML library in the workspace and we prefer our own code to a new dependency, and a JSON text view needs none. Import
accepts a flow file exported by another install; it arrives as a safe draft).

## Execution semantics

**Graph.** A deterministic DAG scheduler. A node is *ready* when every incoming edge is *resolved*: *taken* (its source finished
on that edge's port and the edge's `when` is true) or *dead* (the source finished on another port, the `when` is false, or the
source was skipped). A ready node with at least one taken edge runs; with none it is *skipped* and its outgoing edges die (dead
path elimination), so a branch not chosen never blocks a join. `start` has no incoming edge. Dispatch order is the order of the
nodes in the flow, so a run with `max_parallel: 1` is fully deterministic and the property tests rely on it; with more, branches
overlap (fan-out: several edges from one port; join: a node with several incoming edges, or the `join` node: `any`, the default, waits for every branch to be decided and runs if one ran; `all` needs every branch to have run).

**Failure.** A node that fails for good (retries and healing exhausted) follows its `error` port if an edge leaves it (the
failure becomes data: `nodes.<id>.error`), or `on_error: continue` (output is null), else the run fails: nothing new starts,
nodes already running finish, the run ends `failed` (or `blocked` when a heal card waits for you).

**Data.** Every node gets its config with `{{ ... }}` filled in from the run's context: `inputs.x`, `vars.x`, `nodes.<id>.output.<path>`,
`nodes.<id>.status`, `run.id`, `run.trigger`, `item` and `index` inside `foreach`. A string that is exactly one template keeps the
value's type (a number stays a number, a list stays a list); a template inside text becomes text. There is **no code evaluation**.
The expression language (`expr.rs`) has literals, paths, `== != < <= > >=`, `&& || !`, `+ - * / %`, `in`, parentheses and a short
list of pure functions (`len`, `contains`, `starts_with`, `ends_with`, `lower`, `upper`, `trim`, `int`, `float`, `str`, `default`,
`exists`, `is_null`, `join`, `first`, `last`, `keys`, `min`, `max`, `abs`, `round`, `split`, `replace`, `json`). Depth, length and
result size are capped; a path that does not exist is `null`, never an error, except in a node's own required fields.
`{{secret.NAME}}` is only filled in *fields that may carry a credential* (http and remote headers and auth, cli env, grpc metadata), is
read from the person's vault when the attempt starts, is registered with the run's *redactor* and so can never appear in a log, an
output, a heal context or what the model sees.

**Loops.** `foreach` runs a sub-flow once per item of a list (`items`, `flow`, `max_items` (default 100, hard cap 1000),
`concurrency` (default 1), `stop_on_error`) and outputs the list of the sub-runs' outputs. `subflow` runs another flow once. Depth
is capped at 4 and a flow cannot (even indirectly) call itself. Sub-runs are real runs with a `parent`.

**Time.** `delay` waits `ms` or until `at` (cancellable; the wake-up time is stored, so a restart waits only the rest). `timeout_ms`
caps one attempt of any node.

**Approval and notify.** `approval` stops the node in state `waiting` and the run in `blocked` (needs you) until someone decides
(`approve`, `reject`, with the decider's name), or until its `timeout_s` runs out (then `on_timeout`: `reject` or `approve`, default
`reject`). It does not hold a worker thread. `notify` puts a note in the INBOX (and the chat's notification line).

**Idempotency.** A run may be started with an *idempotency key*; starting the same flow with a key that a non-failed run used in the
last 24 hours returns that run instead of a new one (webhook retries, double clicks, a chat "run it" said twice). `http` and
`remote` nodes send an `Idempotency-Key` header, `<run>-<node>`, stable across attempts, unless `idempotency_key: false`.

**Cancellation.** `cancel` sets the run's flag. Sleeps (backoff, delay) end at once, `http` and the runners are given the flag, a
running command is not killed mid-way except by its timeout (a documented limit of the argument-array runner); the run ends
`cancelled` when the nodes in flight return, and its queued and scheduled siblings are untouched.

**Concurrency.** Per flow `max_concurrent_runs` (default 1), global `max_runs` (default 8); a run that
cannot start is `queued` and starts when a slot frees. Per run `max_parallel` (default 4) nodes at once.

**Crash recovery.** The run is written to disk (`<data>/flows/runs/<id>/run.json`, atomic rename) after every state transition. On
start the engine reads the unfinished runs: a node that was `running` and is *idempotent* by its kind (`http` GET and HEAD,
`condition`, `delay`, `notify`-with-key, `script` without side effects) goes back to pending, others become `interrupted` and the run
`blocked` ("restarted while step X was running: rerun it, skip it or cancel"). Waiting approvals and cards stay waiting. Scheduled
runs stay scheduled.

**Logs.** `<data>/flows/runs/<id>/log.txt` (one line per event, secrets redacted), rotated at 512 KB (`log.1.txt`, one generation),
plus a capped copy in the run for the screen (last 400 lines overall, 200 per node). Old runs are pruned per flow (`keep_runs`, default
200, never an unfinished run).

## Retry, timeout, backoff (`retry.rs`)

`RetryPolicy { attempts, backoff, timeout_ms, total_deadline_ms, honour_retry_after, retry_on }`.

* **fixed**: `{ "kind": "fixed", "delay_ms": 500 }`.
* **exponential**: `{ "kind": "exponential", "base_ms": 200, "factor": 2.0, "max_ms": 10000, "jitter": 0.2 }`; the delay before attempt
  *n* (n >= 2) is `min(max, base * factor^(n-2))` plus or minus `jitter` of itself; the jitter is a hash of (run, node, attempt), so a
  run is repeatable and the bounds are testable.
* **per-attempt list**: `{ "kind": "list", "steps": [ {"timeout_ms": 700, "delay_ms": 0}, {"timeout_ms": 400, "delay_ms": 200} ] }`:
  attempt *n* waits `delay_ms` of step *n* before it and may take `timeout_ms` of step *n* (the last step repeats when `attempts` is
  larger than the list; `attempts` defaults to the list length). Example: run 1 may take 700 ms, run 2 may take 400 ms after a 200 ms pause.
* `timeout_ms` (outside the list) is a fixed per-attempt timeout for fixed and exponential; `total_deadline_ms` ends the retrying when
  the next wait would pass it; `Retry-After` (seconds or an HTTP date, in the reply of a 429 or 503) replaces the computed delay when
  `honour_retry_after` (default true), capped by `max_ms` or 5 minutes.
* Attempts are at most 10; delays at most 1 hour.

**What is retried** (`classify_error`, per kind; anything not listed is permanent):

| Kind | Retryable | Not retryable |
|---|---|---|
| `http`, `remote` | timeout, connection refused or reset, DNS failure, 408, 425, 429, 5xx (not 501, 505) | other 4xx, a reply that does not match `expect` |
| `cli` | a timeout; exit codes in `retry_exit_codes`; output matching `retry_patterns` (substring) | other exit codes, program missing |
| `grpc` | `UNAVAILABLE`, `DEADLINE_EXCEEDED`, `RESOURCE_EXHAUSTED`, `ABORTED`, a timeout | `INVALID_ARGUMENT`, `NOT_FOUND`, `PERMISSION_DENIED`, `UNAUTHENTICATED`, `UNIMPLEMENTED`, ... |
| `browser` | navigation timeout, element not yet there (after the step's own auto-wait) | an assertion failed, consent refused |
| `agent`, `script` | a timeout, a provider error marked transient | a budget stop, a refused tool, a script error |

## Node types (`kinds/`)

A registry (`NodeKind` trait: `id`, `label`, `status` (ready, or planned with the reason), `schema` (the fields the properties form is built
from), `ports`, `validate`, `idempotent`, `run`, `classify_error`, `describe`). Store extensions (flow-node packs) will register more.

| Kind | What it does | Config (main fields) |
|---|---|---|
| `start` / `end` | the entry and the exits; `end` maps the flow's outputs | `outputs: { name: "{{...}}" }` |
| `http` | an HTTP call through the agent's HTTP stack (proxy, TLS options, the account's `net_hosts` policy) | `method`, `url`, `headers` (credentials only as `{{secret.NAME}}`), `body` (text or JSON), `expect_status` (list or `2xx`), `extract` (`{name: "path.in.json"}`), `tls` (`ca_file`, `insecure` (off, flagged)), `follow_redirects` |
| `cli` | one command, **an array, never a shell**, in the flow workspace | `command: ["program", "arg"]`, `cwd` (inside the workspace), `env`, `parse` (`text`, `json`, `lines`, `regex` with `pattern`), `retry_exit_codes`, `retry_patterns`, `cli: {id, min_version}` (the CLI catalog requirement); stdin is **not built** |
| `browser` | steps through the browser automation (same step language as `browser_run_script`: goto, click, fill, check, select, press, wait_for, expect, screenshot, snapshot_assert) plus `extract`; the **consent tiers** apply: `read_only` is the default, interacting steps need a per-domain approval that Auto and Autopilot never grant | `steps`, `allow_domains`, `level` (`read_only`, `interactive`), `target` (`headless`, `ide`), `extract` |
| `agent` | an LLM or external-agent step with a prompt, tools allow-list and an output schema, under a budget | `prompt`, `tools`, `output_schema`, `agent` (empty: the configured model), `max_tokens`, `max_cents` |
| `script` | a small sandboxed script (JS in the `iyakki-js` engine, or a WASM tool package from `wtool`), no network, no files, time and memory capped | `lang`, `source`, `package`, `args` |
| `condition` | one expression, ports `true` and `false` | `expr` |
| `switch` | the first matching case's port, else `default` | `value`, `cases: [{when, port}]` |
| `parallel` / `join` | fan-out marker / wait for branches | `mode: all | any` |
| `delay` | wait | `ms` or `at` |
| `approval` | a person decides | `message`, `timeout_s`, `on_timeout` |
| `notify` | a note in the INBOX | `title`, `body`, `level` |
| `subflow` / `foreach` | run another flow once / per item | `flow`, `inputs`, `items`, `max_items`, `concurrency`, `stop_on_error` |
| `remote` | a generic call to another IYAKKI or a service endpoint, with auth from the vault | `url`, `method`, `auth: {kind: bearer|basic|header, secret: NAME}`, `body`, `expect_status` |
| `grpc` | a gRPC call by the **`grpcurl`** program through the runner (JSON in, JSON out) | `address`, `method` (`pkg.Service/Method`), `data`, `reflection` or `proto_path` + `import_paths`, `plaintext`, `metadata`, `deadline_ms` |
| `thrift` | **planned**, not faked: the node can be placed and configured, `validate` says it cannot run yet. The adapter interface (`ThriftAdapter`: `call(endpoint, service, method, args_json, deadline) -> Result<Value, ThriftError>` with `UNAVAILABLE`-style codes mapped like gRPC) is documented in `kinds/thrift.rs`; the first path to build is thrift over HTTP with the JSON protocol, then a CLI client | |

### Browser nodes (QA)

Twenty nodes in the **Browser** palette group act on one browser session that belongs to the run. They are the same engine as the AI IDE's
browser tab and the assistant's tools (the WebKit/WPE helper through `browserjobs::HeadlessSession`; one driver layer, `browsertools::Drv` and
the job queue). The older `browser` node stays for read-only page reads with the consent tiers. How-to and recipes: [`FLOWS-QA.md`](FLOWS-QA.md).

| Kind | Does | Output (for later nodes) |
|---|---|---|
| `br_open` | go to an address, wait for `load`, `idle` or nothing | `url`, `title`, `status` (when the engine says it) |
| `br_screenshot` | a PNG of the viewport, the whole page (screens joined, at most 12) or one element | `path`, `width`, `height`, `name`; the file is an artifact of the node |
| `br_dom` | `page` (serialized DOM), `outer_html`, `text`, `attributes` of a selector, or `summary`; console messages, links, failed images; size cap (default 200 KB, at most 2 MB) | `html`/`outer_html`/`text`/`attributes`, `url`, `title`, `console`, `links`, `truncated`, `bytes` |
| `br_assert` | exists, not_exists, count, text equals / contains / matches, attribute equals / contains, value, visible, hidden, enabled, disabled, checked, address and title equals / contains, no console errors, page contains / not contains; keeps looking for `wait_ms` (2000); `soft` records a failure and goes on | `passed`, `message`, `soft` |
| `br_click`, `br_type`, `br_fill`, `br_submit`, `br_key`, `br_hover`, `br_scroll`, `br_select` | act by CSS selector; click and type use the page's own events, or a real mouse press / real keys with `native: true`; `br_fill` takes a map selector to value (inputs, text areas, selects, checkboxes, radios; a file input is refused) | what was done |
| `br_wait` | for a selector to appear or go, some text to appear or go, an address, or time | the count / text / address |
| `br_extract` | `fields` (selector, or `{selector, attr, all}`) and `rows` (a row selector and columns) into JSON | `data`, each field also at the top, `rows` |
| `br_js` | a JavaScript expression in the page, JSON back, time limited; **administrators only** | `value` |
| `br_frame` | aim later nodes at a same-site iframe, its parent, or the main page; `tabs` lists the one tab | `frame` |
| `br_viewport` | window size: presets phone 375x667, tablet 768x1024, laptop 1280x800, desktop 1920x1080 or 200-1920 by 200-1200 | `width`, `height`, `inner_width`, `inner_height` |
| `br_storage` | a cookie, a `localStorage` or `sessionStorage` value of the open site (a value that looks like a credential must be `vault:NAME`) | `set`, `name` |
| `br_download` | waits for a download the page started and saves it (the flow's own folder, or a folder you may write to) | `name`, `path`, `bytes` |
| `br_compare` | picture against a baseline in `<Flows>/baselines/<flow>/<name>.png` (saved the first time); a pixel diff with a colour tolerance and a percent threshold, a diff picture in red | `passed`, `percent`, `changed`, `baseline`, `current`, `diff` |

**Session and safety.** One session per run, made by the first browser node, closed when the run ends however it ends (done, failed, stopped), before the run
counts as over. Isolated by default (a private profile folder thrown away at the end). `defaults.browser.profile: person` uses the person's own profile and
asks at every run. A run asks **once**, listing the sites its Open URL nodes name, unless the person marked the flow **trusted** (flow settings, never by the
assistant, never by an import); a schedule with nobody to answer fails with a plain message instead of opening a browser. Hosts must pass the account's host
list (loopback addresses are allowed); the page's address is checked again after every node, and a page that ends up on a site nobody approved is left (the tab
goes blank) and the node fails; the engine itself only lets pages navigate to the approved hosts. Acting nodes need the account's browser setting
`interactive`; reading ones work in `read_only`; `br_js` also needs the permission to change settings. Budgets: the account's actions and minutes per run, or the
flow's `defaults.browser.max_actions` / `max_minutes` (at most 1000 and 180). Addresses never carry a user name or password.

**Secrets.** Write a password as `vault:NAME` in `br_type`, `br_fill`, `br_storage` or `br_js` arguments (or `{{secret.NAME}}`). The value is read from the vault at the
moment the node runs, is registered with the redactor (no log, output, history, report or audit line keeps it) and is also removed from every later DOM dump of
that session. Password fields, token-like hidden fields and `data-token` attributes are removed from DOM dumps by the page script, and from extracted values.
**A screenshot is a picture of the screen and can show anything on it, typed text included**; the report says so.

**Honest limits.** The engine has one tab (a link that opens a window opens in the same tab). Console messages and script errors are collected from the moment the QA script
is in the page (the helper has no hook that runs before the page's own scripts), so what a page prints in the first moments of a load can be missed; later messages,
messages caused by a click, timers and errors after the load are kept. A file input cannot be filled (the engine cannot attach files yet; the path is still checked
against the read folders). `status` of a page comes from the navigation timing when the engine has it. Downloads are fetched again by address through the flow's HTTP
stack (host policy and write scope apply), so a download that needs the page's cookies fails. The in-house JavaScript engine used in the tests has no layout: element
screenshots and visibility by size need the WebKit helper (verified with it, see below).

`grpcurl` is a catalog entry in the CLI tile (MIT, github.com/fullstorydev/grpcurl releases, checksum-verified, version from its
`-version` output); the node fails with "grpcurl is not installed" (or "older than 1.8.0"), which is exactly what the healing agent reads to
propose the upgrade through the catalog's installer.
The status code is read from grpcurl's `ERROR:\n  Code: Unavailable` text; the mapping to retry classes is the table above.

### Camera node (`cam_photo`)

`cam_photo {device?, keep?}` takes one picture and gives `path` (a JPEG) and `taken_at` (unix seconds). The camera is private, so it runs only when the **person** switched on
**Camera allowed** in the flow's settings (`defaults.camera.allowed`; the assistant cannot, and a flow from a file, an import or the Store arrives with it off). A run that
nobody started by hand (a schedule, an event) also needs **Camera also when nobody is there** (`defaults.camera.unattended`). The picture is saved in a private folder of the run
(`<flows data>/camera/<run>/`, never in Pictures), the camera-in-use sign shows while it is taken, and the file is deleted when the run ends unless the node has `keep` on. It
goes nowhere except to the model of a later `agent` step with `images`, which is the model chosen in SETTINGS: a cloud model would receive it (the node's log says so; prefer a
model on this computer). When a later step shows the picture to a model that cannot look at pictures, the node fails *before* taking it. Failures are plain: not allowed, nobody
started the run, no camera connected, camera in use. A specific `device` is only checked against the connected cameras; the shell takes the picture with the camera it uses.
Samples: *Is the person sleeping?* and *Camera: is a person there?* (the model answers one JSON object; the prompt treats the picture as untrusted data, describes only what is
visible and never says who anyone is). Two more samples run the data science scripts of `docs/samples/data-science/` through `python3`: they start with a check that fails
with a plain "toolkit not installed yet" message when scikit-learn or OpenCV cannot be imported. Not tested with a real camera or a real Qwen-VL model.

### Node cards, icons and the tooltip card

A node card shows a small vector icon for its kind (drawn by the shell itself, in the kind's accent colour; one per kind, browser steps included: play, flag, globe, terminal,
window, spark, braces, plug, diamond, fork, two lines, merge, clock, tick, bell, nested squares, loop arrow, camera, and variants for the browser steps) and the step's name on
ONE line, cut with an ellipsis only when it really does not fit. The kind and the settings summary are not on the card any more: they are in the tooltip and the step's panel. In a
run, a round badge on the card's top-right corner shows the state (tick, cross, spinner, dot, dash, raised hand for "waiting for you", circular arrow for a retry); problems and
warnings are a triangle with their count. Below 50% zoom a card shows its icon alone. The card's size, its ports and their hit areas are unchanged.

Rest the pointer on a node for about 350 ms (or hold a finger on it for 600 ms, or select it with the keyboard and press `?`) and a **tooltip card** appears near the pointer, kept
inside the window: the full name, the kind, the id, the settings summary, in a run the state, time, attempt and the first lines of the error or output, and the step's
problems and warnings in full. The badge on the corner has its own small card. A move, a click or Esc puts it away. It works in the Design and the Run views.

### Which model looks at a picture

A step that shows a picture to a model (an agent step with `images`, and the camera step's check before it takes a picture) does not assume the chat model can see.
It asks the resolver (`visionmodel.rs`): the step's own model if it can see; else **Settings -> Models -> Vision model** (key `v`, also `iyakki dev config set model.vision PROVIDER/MODEL`;
empty means automatic); else the chat model if it can see; else one is **picked from the providers' own lists** (the lists saved at the last refresh, no network call).
The pick prefers the chat model's provider, then the others, on this computer before on the network; models the server itself labels `vision` before models only the name
says can see (the label is trusted: `Qwen3.5-4B-MTP-GGUF` and `Gemma-4-E2B-it-GGUF` are vision models though their names do not say so); Qwen first, then the rest; the
smallest download first when sizes are known. The pick is kept per chat provider for the session until **Refresh models**. A model the server must still load gets a
generous time (at least 5 minutes) and the log says "Loading the vision model (first use)".

Where the picture goes is always said from the real address of the provider that will receive it: a loopback address is "on this computer"; a private or link-local address,
a `.local` name or another cluster is "on another computer on your network (host)"; anything else is "a CLOUD service (host)". The run log, the permission popup and the
flow settings all use the same words and name the model. A cloud vision model is **never picked silently** for a camera picture: if only a cloud one is found the step
asks for your choice (choosing it as the Vision model is the confirmation). When none is found the step stops with "No vision model found on PROVIDER. Install one, for
example a Qwen vision model, then press Refresh models" as a *needs your choice* failure: the fixing agent is not asked, no attempt is spent, and the card (taken from
the engine's own message) offers **Open Settings -> Models** and **Refresh models** (which reads the lists again and runs the step again). Nothing is downloaded by itself.

### Permission requests

When a step stops for want of a permission only the person can give (today: the camera, and the camera when nobody started the run), the person does not hunt for the
setting. The step's failure becomes a **permission request**:

* **Popup.** If someone is signed in, a dialog opens over whatever is on the screen with the exact text of the permission (for the camera the same text as the
  settings switch: the picture goes only to the model chosen for the flow, a cloud model would receive it, prefer a local one), the flow and step names, and which model
  that is and whether it is on this computer or a cloud service. It is shown for ten minutes; keys 1 to 4, Esc is Not now.
* **Inbox.** Always, popup or not, signed in or not: an entry on the INBOX Approvals page (open it, read the text, choose) and one note (one per run and step; a repeat adds
  nothing). The run stays in "needs you" until it is answered. Dismissing the step's card withdraws the request.
* **Answers.** *Allow this time*: a grant on THIS run only (`Run.grants`), the stopped step runs again (a run that had already ended runs again with the grant); the flow is not
  changed and the next run asks again. *Always allow for this flow*: switches the person's own setting on in the draft (`defaults.camera.allowed`, or `unattended` for the
  separate permission) and publishes that one change as a new version when nothing else is waiting in the draft, otherwise it stays in the draft until published.
  *Not now*: the popup goes, the entry stays. *Block*: `defaults.camera.blocked`, the flow stops asking; undo with "Camera permission requests" in the flow settings.
* **Separate permissions.** "Camera also when nobody is there" is its own request with its own text; Allow this time for it grants only that.
* **Who can answer.** Only the signed-in person's own input in the shell (the popup or the INBOX). No tool, the assistant, the fixing agent, an imported flow, a web API token
  or a paired computer's `iyakki dev input` can: developer-mode input is refused for permission requests ("answer on the device itself"). There is no setting to relax this.
  Every answer is audited (`flow.permission.*`: run, permission, step, who; no secrets).

**Permissions are never "fixed" by the agent.** A step that stops because a permission only the person can give is off (camera not allowed, a run nobody started, a
browser session or trust not given, the account's browser or host rules, a missing vault entry) fails as a *consent* failure: the fixing agent is not asked, no attempt
or budget is spent, and nothing is proposed. The step's card says what to switch on and offers **Open flow settings** (also on the failure banner and in the chat offer).
The healing classifier refuses outright any patch that sets or removes a consent, trust or reach setting (camera, allowed, unattended, trusted, profile, channels, room,
schedule, trigger, runner, hosts, anything under `defaults`), and any step setting the kind does not have: `camera_allowed` on a step is not read by the engine, never
grants the camera (only `defaults.camera.allowed`, set by the person, does) and is an error that stops publishing. A pending proposal is labelled "proposed by the
assistant, not applied until you approve"; an approved one is noted "proposed by the assistant, approved by NAME". A draft that already has such a key: remove it in the
Text view of the flow, or roll the draft back from History to the published version.

## Runners (the real ones are in the agent's `flows/host.rs`)

* **local**: the node runs in a worker thread of this process; `cli` goes through `wsexec::Runner` (argument array, cut-down
  environment, timeout, output cap) in `<data>/flows/work/<flow>/` (a jail: `cwd` is joined and refused if it leaves), `http` and the rest
  in-process.
* **k3s-job** ("Run in sandbox (k3s)"), per node or per flow: the *command* of a `cli` or `grpc` node (and a whole-flow job later) runs as a
  Kubernetes Job by the **same code as `deploy_run_job`** (`K8sTools`): an image per node (`config.image`,
  default the small tools image from settings, or an image from `deploy_build`; the localhost-registry reference rules apply), non-root,
  CPU and memory limits, labels (`iyakki.dev/job: flow`), `activeDeadlineSeconds` = the attempt timeout, logs captured, the Job
  deleted afterwards. It is **refused** on production and read-only cluster profiles, and it never starts a cluster. `http`, `browser`,
  `agent` nodes cannot run in a Job in this version (the editor says so); marking them k3s is a validation error.
  The Job runs in the flow's own namespace `flows-<user>` by default. Per flow (`defaults.sandbox`, edited in the flow settings) the
  namespace may be `workspace:<name>`, which is the IDE workspace namespace `<name>-dev`; any other name is refused (never `kube-system`,
  `default`, or a namespace that is neither the flow's own nor a `-dev` one), and the cluster tools apply the same profile guards as for
  `k8s_teardown`. The image is per node, else the flow's `sandbox.image`, else the tools image from settings.
* **Which programs a command may start (local runner).** An allowlist, not "anything on PATH": the installed CLI-catalog programs, the
  tile's bin folder, and these utilities: curl, wget, jq, grep, sed, awk, cat, head, tail, sort, uniq, wc, date, echo, test, true, false,
  sleep, tr, cut, printf, base64, sha256sum, tar, gzip, git, python3, node. An administrator may add a program (a name or a path) per
  user; the screen asks to confirm and the change is audited (`flow.program.allow`). Always refused, whatever the list says: sudo, su, dd,
  mount, umount, mkfs, chown or chmod on system paths, and `rm -rf` of `/` or a home root. A step with another program fails with a plain
  error naming it; in RUN an administrator sees an "Allow this program" button (confirm first, then the step is retried); anyone else
  just sees the error. The assistant has no tool to widen the list.
* **remote**: the trait slot exists (`ExecRunner` implementations are chosen by `RunnerKind`); the remote runner is a stub that fails
  with "no remote runner is configured".

## Healing: the agent watches (`heal.rs`, agent `flows/heal.rs`)

Policy per flow and per node (`heal`): `mode` is `off`, `suggest`, `auto_fix_simple` or `always_ask`; plus `max_per_run` (default 2),
`max_per_day` (default 6), `cost_cap_cents` (default 50) and `auto_publish_simple` (default **false**).

* `off`: the run fails (or follows its `error` edge).
* `suggest`: the agent diagnoses and proposes a patch, always shown as a card; you approve or reject.
* `auto_fix_simple`: as `suggest`, but a patch the harness classes as simple is applied at once (audited, never silent).
* `always_ask`: the agent is not called until you press "Ask agent" on the card; its proposals always need your approval.

Steps, after all attempts failed:

1. the node is `failed`, the run `healing`; the heal budget is checked (per run, per day, cost); over budget goes to step 6.
2. the **agent** (the built-in assistant, or an external agent through the executor seam) gets a compact **failure context**: the
   node's definition (redacted), the last 3 attempts (inputs and outputs cut to 1 KB, error, 20 log lines), the flow version, the CLI
   versions the node needs and has, and the project-state-style block of the flow (nodes, edges, who consumes this node's output). It answers
   with a structured **patch**: `{ node, ops: [...], reason, diagnosis }` where each op is one of `set_config {path, value}`,
   `set_timeout {ms}`, `set_retry {policy}`, `edit_script {find, replace}`, `cli_upgrade {cli, to}`, `set_env_ref {name, ref}`, or
   something else (which the harness will then refuse or ask about).
3. **the harness classifies the patch, not the model** (`heal::classify`). It is `Simple` only when *every* op is inside the whitelist:
   * `set_config` of `timeout_ms`, `retry`, `url` when only the path or query changes (same scheme, host and port), a header whose name is
     not a credential and whose value is not a secret reference, `expect_status`;
   * `set_timeout`, `set_retry` (a retry with changed backoff);
   * `edit_script` within the node's own script text (`config.source`), for a `script` node, when the result is not larger than 2x;
   * `cli_upgrade` or install of the CLI *the node itself requires* (`config.cli.id`), to a version `>=` the node's `min_version`, from the catalog;
   and the patch touches only the failing node, the flow is not marked `production`, and no output the downstream nodes read disappears.
   *Never simple* (a card, always; some are refused outright): deleting data, any change to credentials or vault entries (`auth`,
   `secret`, header values with `{{secret`), widening permissions or allow-lists (`allow_domains`, `tools`, `net`, `level`, `runner`,
   `image`), running a CLI that is not in the catalog (`command[0]` changes), touching another node or an edge, changing the shape of
   outputs (`extract`, `parse`, `outputs`) that downstream nodes consume, a missing env or secret reference (asks), anything when the flow
   is `production`, a patch with an unknown op. The model's own claim of risk is recorded but ignored.
4. **allowed and simple** under `auto_fix_simple`: applied. Otherwise a **card** is created: inline in the flow view and the run, an
   INBOX item (`flows`, link to the run), and a chat message with Approve / Reject / Edit chips. The run is `blocked` (needs you).
5. An applied patch **makes a new DRAFT version** of the flow (never a published one; author `agent:<name>`, the node carries an
   `agent changed` badge until published and in the version history). If the node is read-only or idempotent, the new node is **tried once
   with the recorded inputs** (a dry run, results not stored); then the failed node is rerun in the run. **Success marks the patch
   `verified`.** Failure counts against the heal budget and the loop goes back to step 2 with the new failure.
6. Out of budget (or an unfixable or rejected patch): the run is `blocked`, with the agent's diagnosis and an **evidence folder**
   (`<flow>/evidence/<run>/<node>/`: the node, attempts, logs, patches; the same idea as the `B-` bug evidence). Nothing is retried on its own after that.

Everything is audited (`flow.heal.diagnosis`, `.proposed`, `.applied`, `.approved`, `.rejected`, `.verified`, `.failed`, `.budget`), and
the model never sees credentials, addresses of the vault, or the secret values (the redactor runs on the context).

## Assist: copilot and autopilot (`assist.rs`, `assist_store.rs`, `assist_run.rs`)

Each flow has an **Assist** setting (flow settings, shown when no step is selected): `off` (default), `copilot` or `autopilot`. It is stored in the flow
(`defaults.assist`, written only when it is not the default, so a flow from before this setting loads as off and its content hash still verifies) and is
read from the **published** version, so switching it on is a published, diffed change. Imported flows, Store templates, samples and flows made by the
assistant start with it off; the assistant's tools refuse to set it (only the person, in the screen). Full design, decisions and limits:
[`development/FLOW-AUTOPILOT-PLAN.md`](development/FLOW-AUTOPILOT-PLAN.md).

* **Copilot.** After a failed run (the newest finished run of a flow, once) the configured agent is asked for a fix of the failing step (the same redacted failure
  context and JSON patch as healing; no tools). The harness classifies the patch, applies it to a copy of the **published** version (never to your draft), checks the
  result (validation, the classifier) and tries the changed step once with the recorded input if it is read-only. The result is a **suggestion** stored with the flow:
  the proposed version, its diff, the reason, the checks, the trial and the guard lines. A strip on the open flow says so (**Review**); the list shows the diff;
  **Approve and publish** puts the change in the draft and opens the ordinary Publish dialog (the store still refuses an agent's change without an approver);
  **Reject** takes it out. Newer, compatible library versions of a step become **upgrade** suggestions the same way (never published by themselves).
* **Autopilot.** The same, and with the flow's *autopilot may publish a tested fix* switch on, a FIX is published by itself when **all** guards pass: the checks and
  the trial passed, the diff is small (`max_diff_nodes`, `max_diff_fields`), no new external effect, nothing protected touched (credentials, trust flags, camera or
  channel permission, schedule, runner, allow-lists, the heal policy, the assist setting itself), no new host, the draft was the published version, and the caps
  (per flow and per account per day, drafts per day, spend per day and per fix) hold. A failed guard turns the publish into a question that carries the reasons.
  The version records `agent:assist` and `policy:autopilot` as its approver, the audit and the bell say what happened, and the version is **watched**: if one of its
  first `watch_runs` (3) runs fails, the previous content is published again, autopilot is paused for `cooldown_hours` (24) and the bell asks you. **Roll back** and
  **Pause autopilot** are one click on the strip.
* **Never automatic**: apply-to-runs (always asks), delete (never), an upgrade (always asks), a production flow. An account switch (`enabled`) turns every flow's
  assist off at once. A fix of a fix is bounded (two published versions in a row by the agent, then it waits for you).

The model is the configured chat model today (a named agent profile with fallback models and budgets is the next slice). Checked with a scripted model, the real
engine and store, and the real host; **not** with a real model or a real unattended run.

## Versioning, publishing, pulling (`store.rs`)

```
<home>/Flows/<flow-id>/
    flow.json            the DRAFT (what the designer edits)
    meta.json            published (the number), tags {name: version}, the draft's base version, changelog entries
    versions/v1.json ... immutable, monotonic integers; each file has a content hash checked on read
    evidence/<run>/<node>/...
<home>/Flows/_library/<node-id>/node.json + versions/vN.json   the reusable node library
```

* **Draft and publish.** Editing changes only the draft. **Publish** (Ask level in the tools, a confirm in the UI, never automatic;
  agent-made drafts need your approval too unless the flow's policy has `auto_publish_simple` and every change since the last publish was a
  *simple* heal) writes `versions/v<N+1>.json`, with an optional label (`1.4.0`), the author (a person or `agent:<name>`), a changelog line
  and the time. A published file is never rewritten. **Rollback** publishes an older version's content as a new version. **Tags** name a version
  (`stable`).
* **Diff** between any two versions (or the draft): node-level (added, removed, changed) and field-level (path, old, new), edges, triggers,
  defaults. Secrets are references, so a diff never shows one.
* **A run pins a version** when it starts (the snapshot is stored in the run). Each node's **`track`**: `pinned` (never changes under a run),
  `latest-published` (at the node's next boundary the run switches to the newest published definition of this node), `compatible` (only if
  the new definition has the same ports and output names; else it stays and shows "update available").
* **Pull an updated node into running and scheduled tasks.** When a node changes in a published version (or a library node gets a new
  version), the flow screen shows "**N running and M scheduled runs use an older version of this node**" with: *Apply to scheduled runs*,
  *Apply to running runs at their next node boundary* (never in the middle of a node: the run keeps a *pending update* that is taken when
  the node is about to start, or dropped if the node already finished), *Leave*. The healing agent may propose the same ("pull the fixed
  node into the other runs that use it") with the same approvals. Incompatible updates (changed ports or outputs) are shown as such and
  need an explicit "apply anyway".
* **Library.** A node saved in the library has versions; flows hold a reference (`lib: {id, version, track}`) and the *usage list* (which
  flows use it, at which version) is computed from the flows on disk.

## Scheduling and triggers (`sched.rs`)

Triggers of a flow: `manual`, `cron` (five fields, lists, ranges, steps, month and weekday names; day-of-month and day-of-week
OR as in cron) with a time zone (`local`, `UTC`, `+05:30` or an IANA name read from the system's zone files), `interval` (`every_s`,
`anchor`), `once` (`at`), `calendar` (fires when a CALENDAR event whose title contains `match` starts; uses the calendar store),
`file_change` (a path polled every 10 s by size and modified time), `on_failure` (another flow's run failed), `event` (an INBOX or named event),
`chat` ("run it" through the assistant), `webhook` (**planned**: an authenticated local HTTP endpoint needs the server's token
scheme and is not routed; the request gate in `flows/webhook.rs` is built and tested (header `X-Iyakki-Timestamp` unix seconds, `X-Iyakki-Signature: sha256=` HMAC-SHA256 of `<timestamp>.<body>` with a vault secret, 5-minute window, replays refused with 409, 30 calls a minute per path, the signature is the run's idempotency key) but no HTTP route calls it yet; the trigger can be declared and shows "planned"; `flow_run` and the idempotency key are the manual substitute).
Each timed trigger has `missed` (`skip`, `run_once`, `catch_up(n)`: what to do for the ticks that passed while IYAKKI was off or the
schedule was paused), `overlap` (`skip`, `queue`, `allow`: when the previous run is still going), `jitter_s`, `enabled`. Pausing a flow's
schedules is one switch; the next five fire times are listed in the SCHEDULE view. The scheduler state (last fire, next fire, paused) is
a file, so it survives restarts. Tests use a fake clock.

**Who ticks.** The engine's tick starts at sign-in and stops at sign-out (`FlowEnv::start_for_login` and `stop_for_logout`, called by
the shell), so schedules run while the account is signed in without FLOWS being open. Nothing starts for an account that has no flows
(the first flow made opens the environment as before). FLOWS shows a "scheduler running" chip in its header and SYSTEM's overview
shows "flows scheduler running" on the load card. Signed out means not running: missed-run policies apply at the next sign-in.
`max_concurrent_runs` defaults to 1 (a flow that sends, pays or deletes must not overlap itself) and, like `max_parallel`, the default
time limit, the overlap policy of the timed triggers, the default heal mode and the sandbox namespace and image, is edited in the
**flow settings** panel that DESIGN shows when no step is selected.

## Security model

* **Policy.** The tile and the tools need the `flows` app in the role (admin, operator and user have it; guests do not). `http` and
  `remote` hosts must be allowed by the account's `net_hosts`; files for `cli` stay in the flow workspace; `browser` follows the
  browser consent tiers and `Policy::browser_agent`; `agent` nodes use only the tools in their allow-list that the account may use.
* **Credentials only from the vault**, by reference, filled in at the moment of the attempt, redacted from every log, output, card,
  evidence file, context and tool answer. A flow file never holds one; `validate` refuses a header whose name looks like a credential
  and whose value is a literal (use `{{secret.NAME}}`).
* **No shell**, anywhere. Commands are arrays; templates are data only.
* **The agent never publishes, never widens its own autonomy, and never changes credentials**; its edits go to drafts and are badged and
  audited (the one exception is a flow whose owner switched on Assist autopilot with automatic publishing: see "Assist: copilot and autopilot", after the healing section); `flow_publish`, `flow_rollback`, `flow_apply_to_runs` and `flow_set_heal_policy` (when widening) are *hard-ask*: they ask every
  time, in every mode, including Auto and Autopilot (`never auto`).
* **Sandbox**: the k3s-job runner (above) is the isolation for untrusted commands.
* Imported flows are validated like edited ones and arrive as drafts; a flow from outside never has heal mode above `suggest` and never
  `production`.

## Node SDK (adding a kind)

Implement `iyakki_flow::NodeKind` in a module, register it in `kinds::builtin()` (or hand a `Registry` more kinds from a Store extension later).
A kind returns its `schema()` (list of `Field { key, label, kind: text|multiline|number|bool|choice|json|secret|list, help, required, default }`;
the properties form is generated from it), `ports()` (for example `condition` has `true` and `false`), `validate()` (list of issues with a
severity), `run()` (given the resolved config, the input and a `NodeCx` with the host, the cancel flag, the timeout of this attempt and a
logger) and `classify_error()` (see the retry table). Test it with `FakeHost`.

## The screen (`apps/flows*.rs`)

Tabs: **DESIGN** (canvas), **RUN** (live run over the same graph), **BOARD** (lanes Scheduled · Running · Needs you · Failed · Done,
filters, search, next runs, run history, run detail), with a header of chips (running n, needs you m, scheduled k), a version chip (draft or
published, "agent changed") and the assistant pane (`ChatApp::with_flows`) at the right (a toggle under 1000 px).

* **Canvas**: nodes are glowing cards with typed ports; click selects, drag moves (snapping to a 20 px grid), drag from an output port to an
  input port connects (type and cycle checks), pan with the middle button, space+drag or the empty background, zoom with the wheel (25% to
  300%), a minimap, the palette (search, categories), the properties panel (a form built from the kind's schema, plus retry, timeout, runner, heal,
  track), validation badges, undo and redo (Ctrl+Z, Ctrl+Shift+Z), copy and paste of nodes (Ctrl+C, Ctrl+V), Delete, arrow keys to nudge,
  Tab to cycle the selection, F1 for the keys, a text (JSON) view that stays in sync, "test this node" with a sample input.
* **RUN**: the same graph coloured by state (grey pending, blue running with moving dashes on its edges, green done, amber retrying with
  "attempt 2/4, next in 400 ms", red failed, purple waiting for you, dim skipped), a timing waterfall, the selected node's inputs, outputs,
  logs and attempts as selectable text, buttons retry, skip, cancel, rerun from here, and the heal card.
* **At 800x480** the canvas is **view and run only** with a note ("design on a larger screen or use the list editor"); the **list editor**
  (nodes in order with their edges as text, edit fields in the properties panel) works at every size. Decision: a list editor is more
  useful than a cramped canvas.
* **Closing and results** (`flows_fail.rs`): every flow view has a visible **Close (Esc)** (RUN too). Esc is one step back: in DESIGN it
  deselects, then closes the flow; in RUN it returns to the screen the run was opened from (never out of FLOWS); on the list and the
  board it leaves FLOWS. A flow saves after every change, so closing never loses work: the status line says "Closed ... saved in its
  draft". Only text that was refused (a field with an error, a text view that is not a valid flow) asks first (`y` closes, Esc or Enter
  keeps editing). **Run (r)** starts the flow; a flow with required inputs asks for them in a form first (the missing one is focused).
  The RUN view has a banner under its header: while going "n of m steps done" with **Stop run (c)**; when done "Succeeded in ..." or
  "Run failed at step X: reason" with **Run again (a)** (same inputs) and **Open log (l)** (the failed step, Log tab).
* **A key hint footer** on every screen and focus (`footer_hint`, also the app's `hint()` for the shell and F1): the list, the canvas
  with and without a selection, the settings form, the text view, the step search, the assistant, RUN and BOARD each show their own keys.
  Empty states say what to do next ("No workflows yet - press N or ask the chat to make one"). The list tags flows whose last run failed.
* **The agent jumps in on failure** (`iyakki-agent::flows::failfix`, `flows_fail.rs`): when the newest run of the open flow failed
  (manual, scheduled or triggered; a scheduled failure shows when you next open the flow) and you have not dismissed it, a calm card at
  the top of the flow says "Run failed at step X: reason" with **Ask the agent to fix (F)**, **Open log (L)** and **Dismiss (x)**.
  "Ask the agent" opens the assistant on that flow, selects the failed run and sends a prepared request: flow id and name, run, failed
  step, the redacted reason and the last 12 log lines of the step as data, the NAMES of the inputs (never values; input values and
  credentials found in the text are removed), and the instruction to call `flow_explain_failure` and `flow_get`, explain, and propose ONE
  fix with `flow_edit` on the open flow. The tools are the ordinary ones: `flow_edit`, `flow_run`, `flow_publish`, `flow_rollback` all ask
  the person (a test pins their risk), so nothing is edited or re-run by itself; Auto approvals are ended for the fix turn. At most
  **3 attempts** per failing streak of a flow (the count is per flow and starts again after a run of it succeeds); the third request tells the
  assistant to stop and say why if the cause is still unclear, and the fourth click is refused with the reason. The assistant never opens
  by itself. If the assistant is **already open** over the flow when you open it (or when it fails while you watch), the assistant says in
  the thread once per failed run: "The last run failed at step X: reason. Want me to look into it...", and the assistant pane shows
  **Yes, fix it** and **Not now**. Dismiss (and Not now) is remembered per run in `<data>/flows/<user>/failure_cards.json` (run ids and
  counts only); a new failed run brings a new card. The offer can be turned off on the list ("Agent help on failure", key `o`; default on).
* One caret, one keyboard owner, selectable text everywhere, no network or blocking work on the UI thread: the engine has its own threads;
  the screen polls a snapshot (every 250 ms while something runs, otherwise on a change) and sends commands over a channel; the graph layout is cached
  and redrawn on a change or a tick only.
* Export and import use the host files dialog like the other tiles (`docs/HOST-FILES.md`).

## The assistant

Tools (`agent/flows/tools.rs`, wired like DASH's): **Safe** `flow_list`, `flow_get`, `flow_runs`, `flow_run_status`, `flow_logs`,
`flow_explain_failure`. **Ask** `flow_create` (a draft from a structured description), `flow_edit` (a patch of a *draft*: nodes, edges, config; it
never touches a published version), `flow_run` (now, or at a time), `flow_cancel`. **Hard-ask** `flow_publish`, `flow_rollback`,
`flow_apply_to_runs`, `flow_set_heal_policy` (only when it widens the agent's autonomy). Inside the tile the chat is given a compact, redacted
picture of the open flow and the latest runs (at most 5,200 characters, no secrets, no addresses of the vault; see "What the assistant is told").

**Flows first.** The chat in this tile is told it works on workflows: the `flow_*` tools come first in its list, "create" always means a flow
(the app-building tools `app_scaffold`, `tile_*`, `app_preflight` and `app_bump_version` are not offered here and refuse if called; apps are
built in the AI IDE), and when a flow is open a request to change it is a `flow_edit` of that flow (a draft; the reply lists the steps
added, changed and removed; publishing is still the person's). A new flow is made only when none is open or a new one is asked for.
Each workflow has its own conversation: switching the open flow (or closing it) switches the chat to that flow's thread, and nothing of one
thread goes into another's request.

**What the assistant is told (`context_text`).** With every message the chat is given, first, the open flow: id and name, version and
state (never published, published as version N, "the draft has unpublished changes"), the selected step and tab, every step with its kind and
retry, the declared input NAMES (never values), the edges and the trigger or schedule; then the newest runs of that flow (status, steps done,
error); then, for the newest failed run, the failed step (id, title, kind), the attempt counts, the redacted reason, the run's input names and
the last 12 redacted log lines; after that the list of all flows and the run counts. The open part is never cut for the rest. Input values
and credentials are removed (the same cleaning as the failure card); the whole picture is bounded.

**Three ways to the same context.** (1) The assistant pane inside FLOWS (button "Assistant", Ctrl+I, F6 to move to it) shares the tile's open
flow. (2) "Ask the agent to fix" on the failure card opens that pane and sends the prepared request through the same path. (3) The ordinary
CHAT tile: when the person leaves FLOWS the tile remembers the flow last open there (id and name only, in memory, per person), and a CHAT
tile opened afterwards works on it: the same flows-first prompt, flow tools and picture, and the flow's own thread. The CHAT header then
shows a chip "Flow: NAME" (in red with "(last run failed)" when the latest run failed) with "x Detach"; detaching forgets the flow. The
record ends at sign-out, when the flow is deleted, on Detach, and after 4 hours without using FLOWS.

**Check it on the device.** Each time a chat starts with flows context the system log gets one line, visible in `dev dmesg` / `dev logs`:
`chat: flows context flow=<id> name=<name> failed_run=<id|none>` (no secrets).

**Missing input: `ask_form`.** `flow_run` refuses a run that lacks a required input and hands over ready fields built from the flow's declared
inputs (`secret` inputs become secret fields). The assistant then calls `ask_form` (title, 1 to 6 fields of kind text, number, choice,
yes-no, secret or path, with required, default, choices and help; lengths and counts are bounded and checked). The chat opens the shared
form dialog: Tab / Shift+Tab or the mouse move between fields, Enter walks and submits on the last field, Ctrl+S submits, Esc cancels. A
missing required value or a bad number or choice shows an error and keeps the form open. The values go back as the next message; Esc sends
`Form cancelled.`. A secret field is masked and goes straight into the person's vault as `flow.<name>`; only `{{secret.flow.<name>}}` is sent
on, and the audit line holds the number of fields only.

**Deleting.** `flow_delete` (a whole flow) and `flow_drop_version` (one published version) are hard-ask like publishing, and the screen has
the same actions (`d` on the list, `x` in the history). The confirmation names the flow and version and what is lost; Enter does not confirm
(`y` does; Esc, `n` and Enter keep everything). A flow with unfinished runs, or a version an unfinished run uses, is refused with the reason.
The current published version (so the only one) is never dropped. Runs keep their own copy of the flow. Audit: `flow.delete`,
`flow.version.drop`, id and name only.

## Store hook

Flow templates and node packs are `extension` items with target `flows` (`ext_kind` `template` or `node`). The registry is ready for them
(`templates::all()` is the built-in list; an installed template is read from the same JSON shape); the Store's own screen is the Store
agent's job. Six templates ship: HTTP health check every minute with an alert, nightly backup by CLI with retry and backoff, build, test and
deploy to a k3s sandbox Job, website change monitor (browser node), gRPC call with deadline and retry (grpcurl), daily report (query, agent
summary, notify), plus four samples: Is the person sleeping?, Camera: is a person there?, Scikit-learn test set and model, OpenCV shapes and motion.

## Files

`crates/iyakki-flow/src/`: `kinds/browser_qa.rs` (the browser nodes), `report.rs` (the QA report), `model.rs`, `retry.rs`, `expr.rs`, `minire.rs` (a copy of the agent's small regex engine; the engine may not depend on the agent),
`graph.rs` (readiness, dead paths), `kinds/` (the registry and the nodes), `validate.rs`, `engine.rs` and `driver.rs` (runs, threads, healing loop),
`runstore.rs`, `sched.rs`, `store.rs` (versions, library, diff), `heal.rs` (patches and the harness's classification), `layout.rs`, `templates.rs`,
`host.rs` (the seam, `FakeHost`, `FakeClock`), `assist.rs`, `assist_store.rs`, `assist_run.rs` (copilot and autopilot). Tests: `crates/iyakki-flow/tests/` (engine, heal, kinds, versions, properties, assist).
`crates/iyakki-agent/src/flows/`: `host.rs` (HTTP, commands, sandbox Job, browser, model, vault, INBOX), `browser.rs` (the run's browser session, approvals, budgets, downloads, baselines), `qa_fixture.rs` and `browser_tests.rs` (a fixture web server and the in-house page engine double), `mod.rs` (environment, building and editing flows,
what the model is told), `tools.rs` (the assistant's tools), `tests.rs`. `crates/iyakki-shell/src/apps/`: `flows.rs` (state, mouse), `flows_keys.rs`,
`flows_view.rs` (top bar, designer, list editor, dialogs), `flows_canvas.rs` (geometry), `flows_paint.rs`, `flows_props.rs` (the form), `flows_run.rs`,
`flows_board.rs`, `flows_fail.rs` (closing, the result banner, the failure card, the key hints), `flows_tests.rs`. Agent side of the failure card: `crates/iyakki-agent/src/flows/failfix.rs`. Assist: agent `flows/assist.rs` (the account's store and the unattended pass), shell `flows_assist.rs` (strip, list, rollback, pause).

## Built, partial, planned

Rows `F-FLOW-01` to `F-FLOW-26` in `FEATURES.md` carry the status and the tests. In short:

* **Built and tested**: the engine, retries and backoff, expressions, versioning, publishing, rollback, the library and pulling updated steps into other runs,
  scheduling, the HTTP, command, gRPC (through grpcurl), remote, script, condition, switch, delay, approval, notify, sub-flow and for-each nodes, the healing
  loop and its classification, the assistant's tools and their risk levels, the thirteen templates, the tile's three tabs.
* **Built and tested with a real page engine**: the Browser (QA) nodes, the QA report with HTML and JSON export, the baseline comparison, the three QA templates (see `FLOWS-QA.md`; the real-WebKit test is `live_login_form_screenshots_console_and_downloads_with_real_webkit`, ignored by default, run here with python3-gi and Xvfb).
* **Partial**: the sandbox Job runner (tested with a fake kubectl), the older `browser` node (tested with a fake host), the WPE helper of the image (the QA nodes were run against the WebKitGTK helper only), agent steps (configured model only, no tools),
  healing by the built-in model only, the Store's screen for flow templates, the tile (never looked at in a real window).
* **Planned, not faked**: Thrift (the node can be placed, refuses to run), webhook triggers, the remote runner, tools and external agents inside an agent
  step, command stdin, YAML text view, a middle-button pan.
* **Not verified**: how the closing, the result banner and the failure card look and feel on the Mac mini and with a real model's diagnosis; a real model's patches, a real cluster, a real browser page, a real grpcurl run, the screens in a real window and on the Mac mini.
