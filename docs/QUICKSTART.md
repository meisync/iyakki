# IYAKKI quickstart: a 15-minute tour

Building and installing instead? See [`QUICKSTART-BUILD.md`](QUICKSTART-BUILD.md) (build, window mode, write to disk, update, cloud).

**For:** someone who has IYAKKI running (the easiest way is window mode, see [`INSTALL.md`](INSTALL.md)).
**You will get:** a signed-in system, a model that answers, an AI IDE project deployed to a local k3s cluster, a table
and a dashboard, a story read aloud, and Claude Code installed from the CLI tile.

Each step ends with **Expected**. If it does not happen, see [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md).
Steps 5 and 7 need extra software (podman or docker, a local k3s, a speech engine); skip what you do not have.

> Alpha note: this tour was written from the source and the rendered screens. A person on a clean machine has not
> walked through it end to end yet, so report anything that differs ([`../CONTRIBUTING.md`](../CONTRIBUTING.md#reporting-issues)).

## 1. Sign in

Run `./run.sh window`. On first launch the setup screen asks for an administrator name and a password (8+ characters,
twice). Press Tab between fields and Enter to continue.

**Expected:** the launcher with a grid of tiles and chips (Ask & Create, Build, Media, Data, Devices & System, and any group you make with the **+** chip; `m` moves the selected tile to another group). The first sign-in
of an administrator adds a few samples (an MCP server, documents, skills). `F1` explains any screen; `o` signs out.

## 2. Connect a model

IYAKKI ships no model. Pick one:

* **Ollama (local or on your network):** install Ollama and `ollama pull` a model. As administrator, IYAKKI finds an Ollama
  on `127.0.0.1:11434` at sign-in and adds it to Settings -> Models. On another machine, open **NET SCAN**, press `s`
  to scan, select the Ollama server and press `a` to add it.
* **OpenAI-compatible server or a cloud provider:** **SETTINGS -> Models**, press `a`, choose the kind (Ollama, OpenAI,
  OpenRouter, Anthropic Claude, Google Gemini, OpenAI-compatible), fill the address (cloud kinds have defaults) and the
  key. Keys go into the encrypted vault, never into a settings file: press `k` on a provider to set or change it.
* Then press `m` on a model row to make it the **chat model**, and `c` to test the chat.

**Expected:** the test answers with text. Models with "thinking" can take a while before the first word.

## 3. Chat

Open **CHAT**. Type a question, Enter to send. `Ctrl+G` switches **Agent** mode on so the assistant can use tools; every
risky call pauses for **Allow once / Allow for this session / Deny** (`Ctrl+Y`, `Ctrl+U`, `Ctrl+N`). Try: "What is the weather in Paris?"
(a read-only lookup, no approval) then "Write a haiku to haiku.txt in my home folder" (asks first).

**Expected:** the answer streams in; the footer shows timing; the write shows an approval card, and after approving the
file exists. Everything is listed in **RUNS**.

## 4. The AI IDE with a sample project

Open **AI IDE**. Press `Ctrl+Shift+W`, choose **New workspace**, name it `hello`. It creates `~/Workspaces/hello` (or a
folder under the data directory in window mode) and runs `git init`. In the chat pane (right) ask:
"Create a tiny Node or Python web server in this folder that answers hello on port 8080, with a Dockerfile."
Approve the file writes. Open a file with `Ctrl+P`. `F1` lists the IDE keys.

**Expected:** the files appear in the tree and open in the editor tabs. `Ctrl+Shift+B` shows the feature board
(reads and writes the workspace's `FEATURES.md`).

## 5. Deploy the hello app to k3s

Needs: `kubectl`, `podman` (or `docker`), and a k3s cluster on this computer whose kubeconfig you can read
([`K3S-DEV.md`](K3S-DEV.md)). IYAKKI never starts or installs a cluster in window mode; it connects to the one
that exists.

1. In the AI IDE press `Ctrl+Shift+K` (cluster view). The header shows the cluster chip (for example
   `k3s · default · reachable`). If more than one cluster answers, **Cluster...** (`c`) lets you choose.
2. Press `d` (**Deploy...**), fill the image name, build folder and port, and confirm.
3. Four steps appear: **Build**, **Get the image to the cluster**, **Apply**, **Check the rollout**. Each shows the real
   command and waits for **Approve** (`y`), **Approve all steps** (`a`) or Cancel (Esc).
4. When the rollout is ready, `Ctrl+Shift+O` opens the **Browser** tab with an "Open app" button for the service.

**Expected:** pods running in namespace `hello-dev`, and the page says hello. When done, **Tear down...** (`T`) stops or
removes it. Pull errors such as "server gave HTTP response to HTTPS client" mean the registry needs a plain-HTTP setting:
press `g` (Registry...) or see [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md#kubernetes-and-deploy).

## 6. A table and a dashboard

Save this as `~/sales.csv` (or any CSV; in window mode drag a file onto the window to import it):

```
day,region,qty,price
2026-10-01,EU,12,9.5
2026-10-01,US,7,10.0
2026-10-02,EU,15,9.5
2026-10-02,US,11,10.0
2026-10-03,EU,9,9.8
```

Open **DATA**, press `o`, choose the file. Click a header to sort; press `/` and type `qty > 10 and region = 'EU'`, Enter;
press `s` for the column summary.

**Expected:** a virtual-scrolled table, a filter that narrows it, a summary with min, max, mean.

Open **DASH** for graphs over Prometheus, Elasticsearch/OpenSearch, ClickHouse or DuckDB (DuckDB: install it from the CLI
tile, then connect it to your CSV). Five dashboard templates are built in. `a` adds a panel; `Ctrl+I` shows the assistant
beside the board.

**Expected:** the board with panels and a time range; without a data source, panels explain what to connect
([`DASHBOARD.md`](DASHBOARD.md)).

**Object storage (S3).** Store, Setup, Cluster: "SeaweedFS (object storage, S3)" and "RustFS (object storage, S3, Rust, young)" install an
S3-compatible store into the cluster (pick one or both). The licence (Apache-2.0) is shown before Install; the keys are generated into your
vault (`svc.seaweedfs.access-key`, `svc.seaweedfs.secret-key`, record `s3.seaweedfs`; the same with `rustfs`). The address works inside the cluster only.
The assistant has `object_storage_status`, `object_storage_install` and `object_storage_remove`; removing keeps the data unless you ask to delete it
([`PLATFORM-SERVICES.md`](PLATFORM-SERVICES.md)).

## 7. Read a story aloud

Needs a text-to-speech engine (for example piper) set in **SETTINGS -> Voice** (`v` to set the TTS command, `y` to test).
Open **TALES**, type an idea (you can say it in words: *a 5-minute funny story in Tamil about a fox*), choose who it is for (Kids, Teens, Adults) and, if you like, the kind and tone, then press `Enter` or **Send**. **Options ▾** has the length, language, characters, pictures and saving; **Model ▾** and **Agent** choose what writes it.

**Expected:** the story appears as it is written, and with a speech engine it is read paragraph by paragraph with a
highlight; `Space` pauses. Without one the story is still written, shown and saved, and the page tells you how to turn
reading on ([`STORY.md`](STORY.md)).

## 8. Install Claude Code from the CLI tile

Needs Node.js 18+ and npm on the computer.
Open **CLI**, press `Tab` to the **CLIs** page, select **Claude Code** and press `i`. IYAKKI shows the exact command
(`npm install -g @anthropic-ai/claude-code`) and the folder it installs into; confirm with `y`. It runs without a shell
and without `sudo`, in a folder under the CLI tile's own data.

**Expected:** the status says it is installed with a version. Press `g` to put the vendor's sign-in command in the
terminal line (it is not run for you), run it in a normal terminal, then in **SETTINGS -> Agents** use Claude Code as
a chat backend (`Ctrl+P` in the chat). Read the terms note first: [`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md#terms-of-service-decision-for-the-owner).

## Removing what Quick install put on the machine

Every item of Store -> Setup can be removed again. Open its tile and press **Remove** (or `r`). A card lists what goes, what stays and, if you tick the box, what is deleted too.

- **Safe by default.** Removing stops the program or the pod and takes away what IYAKKI added (the Deployment and Service, the program and its folder, the Models entry). Your **data stays**: the volume of the item, its database or models, your vault entries and your DASH connections. Installing the item again finds the same volume and the same passwords.
- **Also delete the data** is a tick, off by default. The card says how much it frees and that it cannot be undone. Items that are only their own data (a Linux sandbox, the Kokoro model, the voice pack) are removed only with this tick.
- **Dependants.** k3s and Lemonade cannot be removed while other installed items need them: the card lists them. Tick "Also remove first" to remove them together (their data is kept), or remove them yourself first. Tools that only use another (Node.js, Python) get a warning.
- **Only IYAKKI's own things.** Each target is checked against IYAKKI's namespaces and folders before it is touched; a cluster found on the machine that is not IYAKKI's is never removed.
- **If it stops half-way,** the card says which step failed and offers **Retry**; running a removal twice is harmless.
- **Where the data lives.** The volumes of the cluster are folders under the data volume (`k3s-dev/data/storage`); after an install their reclaim policy is set to Retain, so even deleting a claim by hand does not delete the folder. On a live session without a persistent data volume the card warns that everything is lost at the next restart.
- **From chat:** `quick_install_status` lists the items and `quick_uninstall` removes one. It asks you every time, with the same card, and `delete_data` is false unless you asked for it.

## Where next

[`USER-GUIDE.md`](USER-GUIDE.md) (every tile) · [`FAQ.md`](FAQ.md) · [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md) (accounts, policy, security).
