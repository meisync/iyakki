# Model host on your network (a Mac's GPU, or any other GPU computer)

Status (2026-10-21): built and tested against fake servers on this machine only. **Not yet tried with a real Mac**: see the last section.

This is route 2 of `docs/APP-MODE-AND-APPLE-SILICON.md`: keep macOS on the Mac, run the model there with Apple's Metal, and let IYAKKI (and the pods of its k3s cluster) call it over the network as an OpenAI-compatible endpoint. It works with **every Apple Silicon chip** (M1 to the newest), because Asahi Linux does not support every chip. The same page works for a PC with an NVIDIA, AMD or Intel GPU that runs Ollama or llama.cpp.

IYAKKI **never installs anything on the other computer**. You do, with the commands below; IYAKKI only talks HTTP to the address you type.

Where: Store -> Setup -> AI servers and models -> **Use a Mac's GPU (Apple Silicon)**, or Settings -> Models -> **Mac / GPU host** (key `H`). Administrators only.

How IYAKKI counts an Apple Silicon Mac's memory for models (two thirds up to 36 GB, three quarters above) and when this card is recommended or hidden: [AI-HARDWARE.md](AI-HARDWARE.md). The card is recommended on a machine with no GPU for models and hidden on an Apple Silicon Mac itself.

## 1. What to install on the Mac (pick one)

| Server | Licence | Port | Start it so it listens on your network |
|---|---|---|---|
| Ollama | MIT | 11434 | App: `launchctl setenv OLLAMA_HOST "0.0.0.0:11434"`, then quit and start the app. Terminal: `OLLAMA_HOST=0.0.0.0:11434 ollama serve`. Then `ollama pull <model>`. |
| LM Studio (server mode) | free to use, **proprietary** | 1234 | Developer tab -> turn on "Serve on Local Network" -> Start. Or `lms server start --port 1234 --bind 0.0.0.0`. |
| llama.cpp `llama-server` | MIT | 8080 | `brew install llama.cpp`, then `llama-server -hf <user>/<repo>:<quant> --host 0.0.0.0 --port 8080 -ngl 99` (add `--api-key <secret>`). Metal is used automatically. |
| MLX-LM server | MIT | 8080 | `pip install mlx-lm` in a virtual environment, then `mlx_lm.server --model mlx-community/<model> --host 0.0.0.0 --port 8080`. No login at all. |

`0.0.0.0` means every network card. Prefer the Mac's own network address (`--host 192.168.1.20`, `OLLAMA_HOST=192.168.1.20:11434`) so it listens on that network only. Find the address in System Settings -> Network, or `ipconfig getifaddr en0`; the name is `scutil --get LocalHostName` plus `.local`.

## 2. Security (read this)

These servers have **no login by default**. Anyone on your network who can reach the port can use your Mac's GPU and read what you send.

- Bind to your network address, not to every interface.
- Switch the macOS firewall on (System Settings -> Network -> Firewall) and allow only the server program.
- Use an API key where the server has one (`llama-server --api-key`, LM Studio's token) and type the same key in IYAKKI: it is stored in the system vault, never in the settings file.
- Or put a reverse proxy with a key in front. Never forward the port on your router.
- IYAKKI accepts only private, link-local and loopback addresses (and `.local`, `.lan`, `.home.arpa`, `.internal` or one-word names, which must also resolve to such addresses). An address outside your network needs the word `PUBLIC` typed in the confirmation field; your prompts then cross the internet **without encryption** (the page speaks plain `http://`).

## 3. In IYAKKI

1. Choose the computer (Mac or another GPU computer) and the server; the page shows its steps and licence.
2. **Edit details**: nickname (the provider becomes `mac-<nickname>`, or `host-<nickname>` for another computer), address (`192.168.1.20`, `studio.local` or `192.168.1.20:8080`; the port defaults to the server's), optional API key.
3. **Test**: `GET /v1/models`, then a tiny streamed chat. It reports the models, which one it chose (never an embedding model first), the time to the first words and **tokens per second** (the tokens after the first one over the time after the first one when the server streams, the server's own count when it gives one). Plain messages for: nothing listening (refused), no answer in time, not an OpenAI-compatible server, 401 (needs a key), malformed JSON, an answer far too large (dropped at 1 MiB).
4. **Add to Models**: registers an OpenAI-compatible provider `mac-<nickname>` with `http://<address>:<port>/v1`, the models seen, the default model, and the key reference `vault:mac-<nickname>-key`. In Models it carries the chip "remote GPU: Apple Silicon (Metal)". Several Macs are fine. An existing provider of that name that is not a model host is never overwritten. Then pick it as the chat model in Models (`m`).
5. **Scan this network** (optional): looks for model servers on the **/24 of this machine's address** (at most 254 addresses, never another range), ports 11434, 8080, 1234 and 8000, 300 ms to connect, 1.5 s to answer, 32 at a time, **only when you press it**. It lists what answers; **Use** fills the address in. Nothing is added by itself. (There is no Bonjour browse: no model server announces itself in a standard way.)

## 4. How it fits k3s

Pods in the dev cluster (Lemonade, Open WebUI style apps, your own deployments) cannot see "the Mac" by name. **Publish to cluster** creates, in the namespace `iyakki-ai`, a Service without selector named like the provider (`mac-studio`) plus an Endpoints object with the Mac's address, so pods call `http://mac-studio:11434/v1` (a host *name* becomes an ExternalName Service instead). Both objects carry `app.kubernetes.io/managed-by: iyakki` and `iyakki.dev/model-host: mac-studio`; **Remove from cluster** deletes only those. It uses the managed k3s (`k3s kubectl` with IYAKKI's own kubeconfig) and no other cluster.

**Test from the cluster** runs a small pod (`busybox:1.36`, non-root, no privileges, no service account token) in `iyakki-ai` that resolves the name and fetches `/v1/models`, reads its log, and deletes it. The first run pulls the image, so the cluster needs internet access for that one time. Pods reach a LAN machine through the node (flannel masquerades their traffic): the Mac sees the node's address.

Network policy: k3s has none by default, so pods may call any address their node can reach. If you add a default-deny egress policy in `iyakki-ai`, allow the Mac, for example:

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata: {name: allow-mac-studio, namespace: iyakki-ai}
spec:
  podSelector: {}
  policyTypes: [Egress]
  egress:
    - to: [{ipBlock: {cidr: 192.168.1.20/32}}]
      ports: [{protocol: TCP, port: 11434}]
```

Check by hand: `kubectl -n iyakki-ai run t --rm -it --image=busybox:1.36 --restart=Never -- wget -qO- -T 5 http://mac-studio:11434/v1/models`. Endpoints is used (not EndpointSlice) because it works on every k3s version; newer Kubernetes marks it deprecated but still serves it.

## 5. Metal facts and limits

- The model runs on the Mac's GPU through Metal on macOS; IYAKKI only talks to the HTTP endpoint.
- Nothing leaves your network: the chat goes from IYAKKI to the Mac and back. The server may download models from its own source when you pull them.
- If IYAKKI runs as a virtual machine on that same Mac, use the address of the host as the virtual machine sees it (the VM's gateway address). The VM cannot use Metal itself. (The light-VM route is documented in `docs/APP-MODE-AND-APPLE-SILICON.md` route 3 and **not built**.)
- Speed depends on the model, its quantisation, the chip and the memory bandwidth: the Test button measures it.
- Unified memory limits the model size: model file + context (1 to 4 GB) must fit in the Mac's memory minus about 4 GB for macOS.

| Memory | What usually fits (4-bit text models; a guide, not a promise) |
|---|---|
| 8 GB | up to about 3 to 4 GB of model: 3B to 4B models. Close other programs. |
| 16 GB | up to about 10 GB: 7B to 14B models. |
| 24 GB | up to about 18 GB: 14B to 24B models. |
| 32 GB | up to about 26 GB: 30B-class models with a long context. |
| 64 GB | up to about 56 GB: 70B models or large mixture-of-experts models. |
| 128 GB | up to about 116 GB: 100B+ and the largest open models. |

## 6. Troubleshooting

| You see | Usually | Do |
|---|---|---|
| "Nothing is listening at ..." | server not running, or listening on 127.0.0.1 only, or wrong port | start it with the host setting above; check `lsof -iTCP -sTCP:LISTEN -P` on the Mac |
| "did not answer in time" | Mac asleep, other network, or the firewall drops packets | wake it, check the address (`ping`), allow the program in the macOS firewall |
| 401 | server wants a key | Edit details -> API key |
| "not an OpenAI-compatible model server" | another program on that port, or the OpenAI API is off | check the port; Ollama: the app version must be recent |
| "the model did not answer in time" | the first request loads a big model | try again; use a smaller model |
| "model failed to load" | model larger than the free memory | see the memory table |
| Test works but pods fail | Service not published, or a default-deny policy | Publish to cluster; see the policy above |
| "not on your local network" | a public address or name | use the LAN address; or type `PUBLIC` knowingly |

## 7. Not verified (needs a real Mac)

Tested with fake servers on loopback: addresses, the test button outcomes, tokens per second arithmetic, Models registration and the vault key, the scan target list and a loopback scan, the Service and Endpoints manifest (golden), the cluster calls (scripted kubectl), and the page at four screen sizes. **Not tried:** a real Ollama, LM Studio, llama.cpp or MLX-LM on a Mac; the exact flags and menu names of those programs (they change between versions: check each program's own documentation); real tokens per second; the macOS firewall prompts; a real k3s pod reaching a real Mac (the busybox pull and flannel masquerade); a Mac that sleeps.

Code: `crates/iyakki-agent/src/modelhost.rs` (rules, network code, manifests) and `crates/iyakki-shell/src/apps/settings_modelhost.rs` (the page).
