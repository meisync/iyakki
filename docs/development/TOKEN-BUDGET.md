# Token budget of the chat agent

Why: on a local model a request of 10k tokens costs seconds of prompt processing, and a browsing run sends it again at every step (one real run showed `last run: ~99.0k tokens` for a short request). This page says where the tokens of one request go, what the routing does about it and how to read the numbers.

## Where the tokens go

Every model request carries four parts. They are estimated with the estimator of the usage ledger (`crates/iyakki-agent/src/usage.rs`: 4 characters a token; `agent::estimate_tokens` for messages, name + description + schema for a tool).

| Part | What it is |
|---|---|
| system | the compact system prompt, the chat's working folder line, and in an AI IDE workspace the project state |
| tools | the definitions of the tools offered in this request |
| history | the person's messages and the assistant's text and tool requests |
| outputs | results of tool calls that sit in the history |

Each model turn writes one log line, and the chat footer shows the same numbers after a run (`tools: 19 of 134 · request ~3.7k (system 1.1k, tools 2.3k, chat 0.1k, outputs 0)`; in the AI IDE the footer tooltip carries the whole line):

```
chat: tokens system=1095 tools=2321 history=12 outputs=0 tools_offered=19 of 134
```

`last run: ~99.0k tokens` in the footer is the sum over all steps of the run: every step resends the whole request, so the size of one request times the number of steps is what a run costs.

## Measured (admin account, screen and full browser engine present, 134 tools = 17.3k tokens)

Run `cargo test -p iyakki-agent --lib tokenbudget -- --nocapture` to reproduce (the `TOKENS` lines). Tool definitions only; the system prompt adds about 1.1k tokens and the TOOLS AVAILABLE NOW block about 0.3k in every case.

| Context | Tools offered, before | Tokens, before | Tools offered, after | Tokens, after | Budget (test) |
|---|---:|---:|---:|---:|---:|
| plain chat ("what is the weather in chennai") | (not counted) | 10.8k | 19 | 2.3k (13% of all tools) | 2.6k |
| Flows (chat opened from a flow, "why did my flow fail") | (not counted) | 10.8k | 35 | 5.4k (31%) | 6.0k |
| Browser ("open gemini in the browser and ask it ...") | (not counted) | 13.1k | 46 | 6.3k (36%) | 7.5k |

"Before" is the lazy-group routing that already existed (web, files, pictures, vault and everything it did not know were always offered, and flows, dashboards, tables, app authoring, cluster and workspace tools were never hidden). The test checks that each routed list is at least 50% below all tools and inside its budget.

## What the routing does

* Tools belong to groups (`toolblock::Group`): web, files, vault and extra (installed packages and MCP servers) are always offered; browser, media, radio, calendar, pictures, system, flows, dash, data, cluster, workspace and build are offered when
  * the chat's context needs them (a flow open in the chat: flows; a dashboard: dash; an AI IDE workspace: workspace, cluster, build, system, data),
  * the last three messages of the person mention them (intent words), or tools of the group were used in the last turns (so "yes, do it" keeps the topic),
  * or the model asks with `more_tools(group)`, or simply calls a tool of a group that is not offered yet (the group is loaded on the spot).
* The TOOLS AVAILABLE NOW block lists every group, so the model knows what it can ask for: nothing becomes unreachable. A test checks that every tool is in an always-on group or in the `more_tools` enum.
* The always-on tools come first in the list, so the start of the request stays the same when a group is added (provider prompt caches work on the prefix).
* Policy is unchanged: the account's policy decides what exists, routing only decides what is offered. Risk, hard-ask and approvals belong to the tool and the call, not to the route.
* Settings -> Models -> `T` (administrator): Tool routing on (default) or off (every tool on every request).

## Not done yet (next slice)

* Group-specific rules out of the compact system prompt (the k8s and browser paragraphs are about 400 tokens), included only with their group.
* Shorter descriptions of the largest tools (flows and browser tools are about 150 tokens each), and head+tail shortening of tool outputs after the turn that used them (today: results over the cap are cut to 300 characters once the history passes 6000 tokens).
* The numbers for history and outputs of a long browsing run, measured on the Mac.
