# K8S: pick a cluster and a namespace, then explore

The **K8S** tile (System group, subtitle *Explore the cluster*) is a stand-alone cluster explorer. It is the same kind of
view as the K8S tab of the AI IDE ([`AI-IDE.md`](AI-IDE.md)), without a workspace: you choose the cluster, then the
namespace, then look at pods, deployments, services, events and the rest. It is **read-only until an administrator turns on
Allow changes**.

It is built on the IDE's pieces and does not duplicate them: the cluster client (`iyakki_agent::k8s::Kube`, run through
`wsexec`: an argument array, never a shell, the managed `k3s kubectl` when the machine has no `kubectl`, vault-held
kubeconfigs written to a 0600 RAM file for the one call and removed after), the plain-English failure texts
(`ide_k8s::banner_for`), the pod parsing and the explanations of why a pod cannot start. The new engine is
`iyakki_agent::k8sexp`; the screens are `apps/k8s_tile.rs` (state and keys) and `apps/k8s_tile_ui.rs` (drawing). Cluster
profiles and the managed dev cluster are described in [`K3S-DEV.md`](K3S-DEV.md).

## Who has it

The roles that have the AI IDE and so its cluster tab: administrators, operators and users. Guests do not. It is
remote-safe in Cloud mode ([`CLOUD-MODE.md`](CLOUD-MODE.md)): it needs no sound, camera or display of its own, so it stays
on a server. **Allow changes is for administrators only.**

## The cluster

At the top, the cluster box lists

* **Dev cluster (managed k3s)**, the cluster IYAKKI sets up in Settings → Dev cluster, with its state as a chip:
  *Running*, *Starting*, *Waiting for the network* (the cluster starts by itself as soon as there is a default route),
  *Stopped*, *Failed* or *Not set up*. When it is stopped the tile says so in words and shows a **Start** button (the same start
  as Settings → Dev cluster, so the same checks and the same audit line). Nothing is asked of a cluster that is not running.
* **Your saved kubeconfig profiles** (the ones you add in Settings → Dev cluster, with the kubeconfig kept in your vault or the
  system vault). Only their names are shown; the kubeconfig is never printed, logged or passed on: it becomes a temporary file
  only while one `kubectl` call runs.

The last cluster you used is remembered, and so are, for each cluster, the last namespace, the last kind and the Allow
changes switch (`k8s-tile-<user>.json` next to your other settings; no kubeconfig content in it). Another person starts clean.

### Several clusters

When the account has more than one cluster (Settings → Clusters), the tile shows the **active cluster's name** with a switcher (status dots, local or remote, a GPU badge when known, "Manage clusters..."). The tile's rows are the account's clusters (the managed k3s first, then the others; kubectl gets that cluster's kubeconfig and context); choosing one makes it the account's active cluster, and a change made in Settings → Clusters or the AI IDE is followed within a second. The confirmation card, the Allow changes message and every audit line of a change name the cluster (`Delete pod web-crash from cluster "gpu-box" (remote)`). A production-looking name stays read-only for the tile. Switching only changes what the tile looks at; nothing is moved or deleted. With one cluster nothing changes and no switcher is shown. See `docs/K3S-DEV.md`, "Clusters".

## Namespaces

*All namespaces* and every namespace, each with a **pod-count badge** and a **red dot when something in it is failing**
(CrashLoopBackOff, ImagePullBackOff, a failed pod, …). The list comes from one call (`get namespaces,pods -A -o json`); when an
account may not list pods everywhere, the namespaces are listed without badges. Type to filter (`/` when the list has the focus),
Esc clears the filter. On a wide window the list is the left column; on a medium or phone-sized window it is a drop-down (key `n`).

## What you can look at

Pods, Deployments, StatefulSets, DaemonSets, ReplicaSets, Jobs, CronJobs, Services, Ingresses, ConfigMaps, Secrets,
PersistentVolumeClaims, Events, Nodes, Namespaces, ServiceAccounts and *Custom resources* (the CRDs of the cluster; their
instances are not browsed). Each kind has its own columns, for example

| Kind | Columns |
|---|---|
| Pods | name, ready, status, restarts, age, node, IP |
| Deployments | name, ready (ready/desired), up-to-date, available, age |
| Services | name, type, cluster IP, ports (`80/TCP→8080:30080`), external address, age |
| Events | type, reason, object, message, age; newest first, warnings in amber |
| Nodes | name, status (cordoned nodes say so), roles, version, CPU and memory capacity, CPU and memory use |

Node usage comes from `kubectl top nodes`, which needs metrics-server (the local k3s runs it; a remote cluster may not); without it the usage columns show *—* and the
tile says why. Narrow windows drop the least important columns first. Click a column title to sort (again to reverse), press `/`
to filter (every word has to match some cell), and read the count line over the table (`12 pods · 2 not ready · shop`).
Status colours: green ready, amber waiting, red failing, grey finished.

**Limits.** One `kubectl get ... -o json` per list, at most 5000 rows kept in memory, the first 500 shown with
*N more: refine the filter*; a cluster whose answer is bigger than 4 MB gets a plain message to pick one namespace. The list is
refreshed every 5 s (namespaces every 10 s) **only while the tile is on screen** (not in a background pane or behind another
screen), and it slows down to three times the time a call took when the cluster is slow (at most one minute).

## The details of one object

Select a row. The details show, in tabs:

* **Summary**: the key fields in plain words (a pod's status in words, what a failure means, who owns it, its containers).
* **YAML**: read-only and coloured, scrollable, with **Copy**. **Secrets show `••••••` for every value**; **Reveal (asks first)**
  shows them after a confirmation card, and the reveal is recorded in the audit log by name only. Values are never written to a
  log, the audit trail or a list, and the copy button copies what is shown (dots, unless revealed). The
  `last-applied-configuration` annotation, which holds a copy of the values, is never shown.
* **Events** for that object, newest first.
* **Related**: pod → owner → Deployment, the Services that select it, its node; Service → its pods and endpoints; workload →
  pods and services in front; node → its pods. Click a line to go there.
* **Logs** (pods): a container picker, **Previous** (the last run of a restarted container), the last 100 / 500 / 2000 lines,
  **Follow** (new lines only, every 2 s; the buffer keeps at most 5000 lines, the oldest fall off), **Pause**, **Wrap**, search with
  highlighting (`/`), **Copy** and **Save** (the desktop window's save dialog, or a file in your home folder).

**There is no Exec.** The tile has no terminal. **Copy exec command** puts `kubectl -n <ns> exec -it <pod> [-c <container>] -- sh`
on the clipboard for the CLI tile. Port-forward is not part of the tile either: use a Service's external address, or the
developer mode's `dev forward`. To change manifests, **Open in AI IDE** (shown when you have a workspace) goes to the IDE's
deploy flow; the tile does not apply YAML.

## Changes (Allow changes)

The switch in the header is **off by default**, **remembered per cluster**, and **only administrators** can turn it on. A cluster
profile whose name looks like production (`prod…`) refuses it. With it on, the details offer, for the kinds that make sense:

| Button | Does | Asks |
|---|---|---|
| Restart | `rollout restart` of a Deployment, StatefulSet or DaemonSet (then waits up to 20 s for the rollout to report) | a card |
| Scale | `scale` to the number you pick (arrows, `+`, `−`, PageUp/PageDown; 0 to 1000) | a card; 0 warns that nothing will serve |
| Delete pod | `delete pod`; says whether a controller recreates it or the pod is gone for good | a card naming the pod |
| Cordon / Uncordon | `cordon` / `uncordon` a node | a card |
| Delete | any other namespaced kind (not Pods, Nodes, Namespaces, Events or CRDs) | **type the name** |

Every name is checked as a DNS-1123 name before it reaches an argument array (`kubectl -n <ns> <verb> <kind>/<name>`: no shell, no
string building), the namespaces `kube-system`, `kube-public`, `kube-node-lease` and `default` cannot be deleted, and the switch
is checked again when you say yes. Each change is **audited** (who, cluster, namespace, kind/name; never a value; the result is a
second line `….ok` / `….failed`), runs **on a thread with a 30 s limit**, shows its output live in a small panel and ends with a
message (toast). The managed `kubectl` path is the only path: a cluster other than the managed one is reached only through the
profile you chose.

## Layout and keys

* **Wide** (1000 px or more of logical width): namespaces and kinds | resources | details. **Medium** (620 or more): resources |
  details, with the cluster, namespace and kind in drop-downs. **Narrow** (a phone, or half of a small screen): one page at a
  time, the list first, **Back** from the details. It follows the size of its pane, so it works in a 50:50 split.
* Mouse wheel and two-finger trackpad scroll the list or text under the pointer; clicks work on rows, titles and buttons.
* Keys: Up / Down / PageUp / PageDown / Home / End move in the list that has the focus (Tab changes it); **Enter** opens the
  selected object (or chooses the namespace or kind); **/** filter (Esc clears it); **n** namespaces; **k** kinds; **l** logs;
  **y** YAML; **s** summary; **e** events; **o** related; **r** refresh; **L** cluster log; **c** copy what the details show; **x** copy the exec
  command; **w** wrap; in the logs **f** follow and **p** pause; **Esc** goes back one step and then leaves the tile. No number keys.

## When something is wrong

The tile never shows a blank page. A stopped, starting, not-set-up or failed dev cluster is explained with a button; a missing
`kubectl` says how to get one; an unreachable cluster, a forbidden account and a failing call have their own sentences (the same
as the AI IDE's), with **Refresh**.

### When the cluster does not answer for a moment

A cluster that is busy (the same computer runs k3s with its pods, the browser and the shell), starting or restarting fails for a
second or two and then works. The tile and the AI IDE's K8s panel share the code that handles this (`iyakki-agent::k8sretry`):

* Each failed `kubectl` call is judged **transient** (timed out, killed, no message, `EOF`, connection reset, `TLS handshake timeout`,
  `context deadline exceeded`, `etcdserver: ...`, "the server is currently unable to handle the request", a 5xx of an aggregated API such
  as metrics, and a refused connection while the managed k3s starts, just changed state or answered before) or **persistent** (no
  kubectl, no kubeconfig, forbidden or unauthorized, a name that does not resolve for a cluster that never worked, a bad context,
  anything unknown).
* A transient failure is tried again up to three times inside the refresh (after 0.7, 1.5 and 3 s). If it still fails, the feed keeps
  trying in the background after 2, 4, 8 and then every 15 s. A call that works is still exactly one call; two calls for the same list
  never run at once; the screen never waits.
* **Refresh** (the button or **r**) ends a wait at once, tries now and starts the delays again.
* While it fails, the **last good list stays on screen** with a dim line: "Updating... (the cluster did not answer, retrying), data is
  N s old". Only after about 25 s of failing (90 s while the managed k3s is starting, with "k3s is starting" in plain words), or at once
  for a persistent failure, the banner replaces the list. It names the real error (first line of what `kubectl` said) and adds "Last
  worked 40 s ago. Retrying every 15 s. Press Refresh to retry now."
* **Show log** (or **L**) opens the **cluster log**: the last 50 kubectl problems with time, the call (without kubeconfig path, context
  or credentials), duration, exit code, first line of stderr, the verdict and the try number. **Copy** (or **c**) puts it on the
  clipboard. It lives in memory only; each failure episode also writes one line to the agent log.

## Not done

* Exec / a terminal, port-forward, applying YAML, editing objects, browsing custom resource instances, a graphical node/pod map.
* The Podman variant of the managed cluster is reached through `podman exec`; it has not been tried on a real machine.
* Everything above was tried against a fake `kubectl` with canned answers. Nothing here has been run against a live cluster; in
  particular the transient-failure handling has not been seen on a real busy machine (copy the cluster log when it shows).
* The AI IDE panel has the same retries, stale data and banner text but no Show log button yet (the log is kept; the tile shows it).

## Tests

`iyakki-agent::k8sretry::tests` (the classification table, retries, waits, cancel, backoff, window, log, feed on a fake clock) and
`iyakki-agent::k8sexp::tests` (parsing for every kind, pods in every phase, ages, namespace badges, sorting and filtering,
Secrets, logs and the follow buffer, the exact argument arrays of every action, name validation, the feed and its back-off) and
`iyakki-shell::apps::k8s_tile::tests` (the screens with a fake cluster: picker, state cards, namespace and kind choice, one call
per refresh, details, transient failures (stale list, banner after the window, log, Refresh ending a wait), reveal, logs, role and switch gating, confirmation flows, layout at 800×480, 1280×720, 1920×1080, 360×740
and in a pane with every click region inside the app, wheel and trackpad scrolling, keys), plus the tile registration tests in
`shell_k8s_tests.rs`.
