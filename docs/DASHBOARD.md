# DASH: dashboards over pluggable data sources

DASH (Work group) draws graphs and tables from systems you already run: Prometheus, Elasticsearch or OpenSearch,
ClickHouse, DuckDB, Neo4j and the k3s cluster of this machine today, more later. A dashboard for the local cluster is there from the start (see "Local cluster (k3s)"). The assistant works on the open dashboard beside it. Everything the tile does is
read-only.

```
 screen (apps/dash.rs, dash_charts.rs, dash_query.rs)     assistant tools (agent/dash/tools.rs)
        |  worker threads, poll                                  |  same Env, same connectors
        v                                                        v
   iyakki_agent::dash   Env { user, policy, vault, runner, transport }
        engine (variables, step, run a panel) ---> DataSource trait ---> prom | es | clickhouse | duckdb
        model, templates, store, session (undo)                      QueryRequest -> Frame     (serde both ways)
```

## The seam (`crates/iyakki-agent/src/dash/connector.rs`)

```rust
pub trait DataSource: Send + Sync {
    fn id(&self) -> &str;  fn kind(&self) -> &str;  fn language(&self) -> Lang;     // PromQl | Sql | EsDsl
    fn test(&self, cancel: &Cancel)   -> Result<TestInfo, DashError>;              // reach it, prove the credentials
    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError>; // indices, metrics, tables, columns
    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError>;
    fn starter(&self, target: &str) -> String;                                     // a first query for a table or metric
}
```

* `QueryRequest { text, target, range: {from_ms,to_ms}, step_ms, limit, timeout_s }` and `Frame { columns (name and type:
  int, float, str, bool, time), rows, series (name, labels, points as (ms, value)), meta (truncated, took_ms, notices,
  executed text) }` are `serde`. That pair is the entire wire protocol: a remote backend (see
  `docs/EMBEDDING-AND-BACKEND.md`) only has to carry those two types and a `SourceConfig` (below).
* `Cancel` is a shared flag the screen sets when you leave or change the question; connectors look at it before and after
  each call and the caller drops a late answer. A request already in flight ends by its own timeout.
* `DashError { kind, message, fix }`: kinds are config, network, auth, bad query (the server's own message), read-only,
  limit, parse, missing program, cancelled, server. The text says what to do: "connection refused (Nothing is listening at
  that address and port...)", "the server refused the sign-in (HTTP 401) (Check the user name and the vault secret...)".
* `SourceConfig { id, kind, name, values, secrets }`: `values` are plain settings, `secrets` map a field to a **vault
  reference**. The form for each kind is generated from `KindInfo.fields` (`registry::kinds()`).
* Limits that every connector applies (`connector.rs`): 10,000 rows, 200 series, 11,000 points per series, 8 MB of answer,
  30 s default and 120 s at most per request.

## Connectors

| Kind | Talks to | Query | Browse | Notes |
|---|---|---|---|---|
| `prometheus` | `/api/v1/query`, `query_range`, `label/__name__/values`, `series`, `status/buildinfo` | PromQL | metrics, then labels | step from the range and the chart width, never more than 11,000 points; also VictoriaMetrics, Mimir, Thanos |
| `elasticsearch` | `POST /<index>/_search`, `_cat/indices`, `_mapping`, `/`, `_cluster/health` | the search body as JSON; the index in "Index / table / file" | indices, then flattened fields with types | hits become a table; `date_histogram` and `terms` aggregations (nested too) become tables and one series per term; builders for "over time" and "top values"; also OpenSearch. ES\|QL is not built |
| `clickhouse` | HTTP interface, `POST /?default_format=JSON&readonly=2&max_execution_time=...&max_result_rows=...` | SQL | databases, tables, columns (named parameters) | use a user that is read-only on the server too |
| `kube` | the local k3s cluster through the managed kubectl (see "Local cluster (k3s)") | the name of a view | the views | no address, no secret; fixed read-only `get`s |
| `duckdb` | the `duckdb` command (see below) | SQL | tables of a database file, or the data files | CSV, Parquet, JSON through `read_csv_auto('/full/path')` and friends |

Planned, designed for by the seam: PostgreSQL, MySQL, SQLite, Loki, InfluxDB, a Grafana-style JSON data source, CSV and
Parquet files through `iyakki-data`, other Kubernetes clusters, plain HTTP JSON.

### DuckDB: the CLI, not a linked crate (decision)

Two ways were weighed.

**(a) The `duckdb` command through the argument-array runner (`wsexec::Runner`), chosen.** `duckdb -json -batch -no-stdin
-init /dev/null [-readonly FILE] -c "SQL"` prints the rows as a JSON array. The flags are those of the DuckDB command-line
documentation (`-json`, `-batch`, `-no-stdin`, `-init`, `-readonly`, `-c`); **they were not run against a real DuckDB in
this work** (none was installed), so the first run on a machine with the program is the check
(`cargo test -p iyakki-agent --lib dash::live -- --ignored` with `IYAKKI_LIVE_DUCKDB=<csv or parquet file>`).
Why: no linking and no build cost, nothing of DuckDB's size in the OS image, a crash or a runaway query is a child process
we can time out and kill, and the licence stays MIT and out of our binary. The program is installed from the **CLI tile**
(catalog id `duckdb`, MIT licence, downloaded over HTTPS from github.com/duckdb/duckdb releases, no checksum published) and
only when you ask: a source whose program is missing says "DuckDB is not installed" and points to the CLI tile. Cost: a
process per query (tens of milliseconds), no connection reuse, and the output is JSON (types are inferred from the values).

**(b) The `duckdb` crate behind a cargo feature, default off, not built.** It is MIT, but it bundles the DuckDB C++ sources
(`libduckdb-sys`), which add a long compile (tens of minutes on a laptop) and tens of megabytes. It would give typed results
and a connection that lives. If wanted later, add a second `DataSource` for the same kind behind the feature; nothing above the seam
changes.

## Security model

* **Read-only, checked before anything is sent** (`dash/readonly.rs`).
  * SQL (`sql_read_only`): comments and quoted text are removed first (`--`, `#` for ClickHouse, nested `/* */`, single,
    double and back-tick quotes, doubled quotes), then the text must be **one statement** that starts with SELECT, WITH,
    SHOW, DESCRIBE, EXPLAIN, VALUES, FROM or TABLE, and no word anywhere may be INSERT, UPDATE, DELETE, DROP, ALTER, CREATE,
    TRUNCATE, ATTACH, DETACH, GRANT, REVOKE, OPTIMIZE, RENAME, KILL, MERGE, UPSERT, PRAGMA, INSTALL, INTO, OUTFILE, SET or
    SYSTEM (REPLACE, TRUNCATE and SET are allowed as function calls; `system.tables` is allowed in ClickHouse). ClickHouse
    table functions that read files or other servers (`file`, `url`, `s3`, `remote`, `executable`, `mysql`, ...) are refused,
    and so are DuckDB's `read_text`, `getenv` and friends. A second statement after `;` is refused. When in doubt it
    refuses: a harmless but odd query may need rewriting.
  * Elasticsearch: only `GET /`, `_cat/*`, `_cluster/health`, `_resolve/index`, `{GET|POST} [/<index>]/_search|_count|_field_caps`,
    `GET [/<index>]/_mapping`; no `_bulk`, `_delete_by_query`, `_update`, `_reindex`, `_doc`, nothing with `..` or `%2e`.
    Index names are letters, digits and `-_.*,:+`.
  * Prometheus: only the query, series, label, targets, metadata, rules and alerts calls; never `/api/v1/admin/*` or `/-/*`.
  * Neo4j (Cypher): one query per request, sent through the Query API as a read transaction (`accessMode: Read`, the server refuses writes) after a text check that refuses CREATE, MERGE, DELETE, SET, REMOVE, CALL, LOAD, FOREACH, DROP, USE and user or server commands. The graph database of Store -> Setup is added as the source `Cluster graph database`.
  * DuckDB: a database file is opened `-readonly`; every file path inside the SQL must be absolute, free of `..`, and inside
    the account's readable folders (`FileRoots`); network addresses are refused.
  * The servers are asked to enforce it too where they can (ClickHouse `readonly=2`).
* **Credentials only from the vault.** A source holds a reference (`uvault:name`, `vault:name` or `ext:provider/path`);
  anything else (a plain value, `env:`, `file:`) is refused when the source is saved, and the add-source form offers a **list of
  the names in your vault**, never a place to type a secret (store it in SETTINGS > Vault, or let the assistant call
  `vault_request`). The reference is resolved when a request is made and sent only as an `Authorization` header (basic,
  bearer, `ApiKey`) or as the mutual-TLS key; it is never in a URL, a log, a dashboard file, a settings file, an error
  message (a rejected reference is not echoed) or what the model sees (`Http`'s `Debug` hides it). Tests assert each of
  these.
* **TLS.** The system roots are trusted; add your own CA file for a private CA; mutual TLS takes a client certificate file and
  a key from the vault; "Skip certificate checks" exists, is off by default, and is described as not safe. A proxy address
  can be set per source. These options are built into `UreqUpstream::with_options` in `iyakki-proxy`.
* **Policy.** The tile and the tools need the `dash` app in the role (admin, operator and user have it, guests do not) and a
  network scope; each source's host must be allowed by the account's `net_hosts`. Files read by DuckDB or imported dashboards
  stay inside the account's folders.
* **The model never sees** an address, a user name or a secret: the assistant's context and tool answers name sources by
  name and kind. The tool results are marked untrusted (they are data) and are kept out of the tool log.

## Services of the cluster (`dash/cluster.rs`)

The Elasticsearch and Prometheus that Store -> Setup puts in the managed cluster are `ClusterIP` Services. Their `*.svc` names exist only
inside the cluster and **nothing listens on this machine's localhost**, so `http://localhost:9200` or `:9090` is refused. A source address may
instead name the Service:

* `cluster://<namespace>/<service>:<port>`, for example `cluster://iyakki-data/elasticsearch:9200` or
  `cluster://iyakki-monitoring/prometheus:9090` (`cluster+https://` for TLS), or
* `http(s)://<service>.<namespace>.svc[.cluster.local]:<port>`.

At every connect IYAKKI asks the managed `kubectl` (`K3sDev::kubectl`) for the Service and its Endpoints, and talks to the Service's **current**
cluster IP (k3s runs natively here, so cluster IPs are reachable from this machine). The answer is remembered for 10 s and forgotten after a
failed connect, so a recreated Service never leaves a stale IP in the source. With no cluster, a missing Service or port, no ready pod, a
headless or ExternalName Service, or a loopback / link-local / multicast IP the source says so in plain words and nothing is sent.

Policy: the account's network policy (`net_hosts`) is asked about the **name** `<service>.<namespace>.svc`, never about the IP it resolves to. An
IP typed by hand is judged as itself, so an arbitrary private IP is not newly allowed. The request goes to an IP literal (no DNS to rebind) and the
HTTP client follows no redirect.

Quick install registers `Cluster Elasticsearch` (id `elasticsearch-cluster`) and `Cluster Prometheus` (id `prometheus-cluster`, an older one with
a baked-in cluster IP is rewritten once when DASH opens) with these addresses. **ClickHouse** works the same way
(`cluster://<namespace>/<service>:8123`) but has no installer item yet; one would need a pinned vendor image, a manifest like the others
(unprivileged Deployment, claim, ClusterIP Service on 8123, a read-only user created for dashboards) and a `register_dash_source` call.

**Repairing a localhost source.** A source that points at `http://localhost:9090` (the old form default) cannot work: the board then shows ONE banner per
source ("Source Prometheus is not reachable: nothing listens on this machine at port 9090", the fix on its own lines) instead of the same red box in every
panel, and the panels show a short state. The cluster is asked (in the background) what could be done, and the button says it:
**Use cluster address** (the Service exists: only the address changes, through the normal save, audited as `dash.source_repaired`, administrators only,
never without the click), **Install Elasticsearch (Store -> Setup)** (nothing in the cluster: Setup opens on that item) or **Remove source** (ClickHouse has no
installer item). The Sources page has the same button for the selected source, **Fix all** (every source whose Service exists, one click; the install/remove
choices stay yours) and **Remove duplicate** (same kind and address as another source). Sources that Quick install saved for every account
(`sources-system.json`) are listed for all accounts; a baked-in cluster IP of `prometheus-cluster` is rewritten to the `cluster://` form whenever DASH or
its Sources page opens.

**Add source** pre-fills the address from what is really there: a matching Service in the cluster (by name, label and port) as
`cluster://...`, else a process listening on localhost, else it stays empty and says why. A failed connect names the cause: nothing listens on
this machine at that port (and where the instance is), `.svc` names only work inside the cluster, no cluster, Service not found, no ready pod,
refused, timeout, TLS, sign-in.

## Local cluster (k3s)

A dashboard named **Local cluster (k3s)** (id `k3s-local`) is saved for every **administrator** account (policy `manage_users`) the first time DASH (or the assistant's
dashboard tools) opens, together with the source **Local cluster (k3s)** (id `kube-local`, kind `kube`). No setup, no Prometheus, no address and no
secret: the source asks the managed k3s (`K3sDev::kubectl`) directly. It refreshes every 30 s.

**What it shows** (20 panels): cluster state, k3s version, nodes ready, cluster age and how long the nodes have been Ready, and whether metrics-server
answers; six counts (nodes not ready, pods with problems, deployments not fully available, warnings in the last hour, services open on the LAN that are
not documented, volume claims not Bound); nodes with roles, version and bad conditions (MemoryPressure, DiskPressure, SchedulingDisabled ...); pods by
phase and by namespace; pods in CrashLoopBackOff, ImagePullBackOff, Pending, Failed, Evicted, not ready or stuck Terminating, each with the reason (the
container's message, the last exit such as `OOMKilled (exit 137)`, or why the scheduler cannot place it) and the restart count; the ten containers that
restarted most; deployments with fewer available replicas than wanted (or `ProgressDeadlineExceeded`); CPU and memory per node (with percent of what
the node can allocate), per namespace and for the fifteen pods using most memory; warning events of the last hour; NodePort and LoadBalancer services,
each marked `documented: ...` (`exposure::DOCUMENTED`) or `NOT documented: open on the LAN`; volume claims with phase, size and, when the kubelet reports
it, usage.

**Views.** A query on a `kube` source is the *name of a view*, nothing else: `cluster`, `nodes`, `node_states`, `pods_by_phase`, `pods_by_namespace`,
`pod_problems`, `top_restarts`, `deployments_unavailable`, `node_usage`, `namespace_cpu`, `namespace_memory`, `pod_usage`, `warning_events`,
`exposed_services`, `pvcs` and the counts `stat_nodes_not_ready`, `stat_pods_with_problems`, `stat_deployments_unavailable`, `stat_warnings_last_hour`,
`stat_exposed_undocumented`, `stat_pvcs_not_bound`. The schema browser lists them; you can build your own panels from them like from any source (the
assistant too: `dash_describe` lists the views, `dash_panel_add` takes the name). The language shown in the editor is "view name (cluster)".

**Read-only, by construction.** The text of a query never reaches kubectl. The source can ask only for what `KubeRead` (`dash/cluster.rs`) lists: `version`,
`get nodes|pods|events (type=Warning)|deployments|svc|pvc -A -o json`, `get --raw` on `metrics.k8s.io` nodes and pods, and one node's kubelet volume
summary (the node name must be a DNS-style name). No `apply`, `delete`, `scale`, `patch`, `exec`, `logs`, `port-forward`, and **no Secrets** (the same rule as
developer mode's `kube` scope). A query that starts with a write word (`delete pods`, `apply -f`, `scale ...`) is refused as a write before the cluster is
asked, and anything that is not a view name is refused as a bad query. From each object only named fields are read: container commands, environment
values and annotations are never copied, and the free text that is kept (statuses, events) is cut to one line and passed through the credential detector.
The source needs the `dash` app in the role like every source. **The cluster is the administrators' own**: the `kube` connector is refused for any other account
(whatever source it saved, also one in the shared system file), and nobody but an administrator is seeded. Everyone else reaches the cluster only through a
dashboard an administrator shared with them (next section), and then sees only the views that dashboard shows.

**When the cluster is down.** Stopped, starting, not installed or not answering: the first panel says `not running` with the reason (`k3s is stopped`,
`k3s is not installed by IYAKKI on this machine`, ...), each table shows one row "cluster is not running", and charts and counts are empty with the same
sentence as a notice. Nothing is red. When the API answers but the failure is something else (for example a permission error), the first panel says
`not answering` and the other panels show the error.

**CPU and memory need metrics-server**, which k3s ships in `kube-system`. While it does not answer (a minute after start, or when disabled with
`--disable metrics-server`) the first panel says `not available: the metrics API is not answering ...` and the CPU and memory panels say the same; the rest
of the dashboard works. **Volume usage** comes from the kubelet's summary and is blank where the kubelet does not report it (volumes backed by a plain
host folder, such as k3s's default `local-path`, usually report none; the panel says so).

**Cost and limits.** kubectl runs as a process (about 20 s at most, 8 MB of answer). Answers are kept for 10 s, so the panels that refresh together make
one call per kind of object. Lists are cut at 20,000 items and tables at 200 rows (pods with problems, volume claims) with a notice.

**Private, and shared by choice, with three roles.** The dashboard belongs to the administrator it was made for and is private to them. In the dashboard list, **Share**
(key `h`) asks for account names (comma separated; the accounts must exist; up to 20) and then asks to confirm, in plain words: the viewers will be able to see
the live state of this machine's k3s cluster (node, pod, namespace and deployment names, restarts and reasons, event messages, services and open ports, CPU and
memory, volume claims); they cannot change anything, read Secrets, logs or a shell, or edit, share or export your dashboard; you can stop at any time. Each account is a **viewer** (looks only; the default) or an **operator** (`bob:operator, carol`); roles can be changed or removed later and the confirmation says what the
role can do. Share with an empty list to stop (also confirmed). The list rows say `shared with bob (operator), carol (viewer)` for the owner and `you are operator` / `you are viewer` for a shared one; audit lines `dash.shared`,
`dash.share_role`, `dash.unshared` and `dash.edited_shared` carry the dashboard id, account names and roles, nothing else.

| | Owner | Operator | Viewer |
|---|---|---|---|
| look at the dashboard (the views it shows) | yes | yes | yes |
| change panels, layout, time range, refresh; saved into the owner's dashboard | yes | **yes** | no (in memory only, never saved) |
| rename, delete, export, source settings | yes | no | no |
| share, re-share, unshare, change roles | yes | no | no |
| queries of their own, other views, Secrets, logs, exec | (the owner's own rights) | no | no |

An operator's save (`Env::save_dashboard`) is checked against the owner's file: the title, description, variables and default source stay the owner's, each panel
must use the dashboard's own cluster source (viewer copies carry `share:...` source ids, mapped back on save) and a view the owner's saved dashboard already
shows; a new view needs the owner. **If the owner changed the dashboard after the operator opened it, nothing is written** and the operator is told to open it again
and redo the change; so the owner is never overwritten silently. Every save of a dashboard keeps the version it replaces as `Dashboards/<id>.json.bak` (one deep, removed
with the dashboard), so the owner can step back from an operator's change and the other way round; the owner's own save does not check for an operator's change in
between, it replaces it and the operator's version is the `.bak`. `shares.json` keeps `with` (everybody) and `operators` (a subset); a file from before roles has no
`operators`, so everybody in it is a viewer, and a damaged role list reads as no operators.
* Only a dashboard that uses nothing but the local cluster source can be shared (the source holds no address and no secret). If the owner later adds a panel on another
  source, the share stops working until she removes it.
* The list is in `<data>/dash/shares.json` (owner, dashboard, accounts) and is read on every list, open and query: unsharing, deleting the dashboard (its shares end
  with it, and a new dashboard with the same id is not shared again), a viewer's account being removed (not shown any more) or the owner's account being removed
  take effect at the next read. A damaged or hand-edited file widens nothing (bad names and ids are dropped).
* A recipient sees the dashboard titled `<title> (shared by <owner>)`, id `share:<owner>/<id>`. That is not a valid dashboard id, so it cannot be saved by the ordinary path, renamed, deleted,
  re-shared or exported (an operator's save goes through the checked path above; an export is refused and would carry no source anyway). The viewer's panels use the owner's source through a wrapper that
  answers only the views that dashboard shows: no query of their own, no other view, and still no Secrets, logs or exec. A viewer may rearrange or change the open
  copy in memory (and the assistant may work on it); none of it is saved or visible to anyone else. The account role (policy) does not matter: an operator in the
  account sense is not given the cluster, only a share is.
* An account without the `dash` app (guests) gets nothing even when named.

**Seeded once.** `dash/seed.rs` saves the source and the dashboard when an account has not been seeded, and records that in
`<data dir>/dash/seeded-<user>.json` (`{"seeded":["k3s-local"]}`; an account that is not an administrator is neither seeded nor marked, so one promoted later is seeded then). It never overwrites a dashboard or a source that already has that id, and after the
first time it does nothing: **a dashboard you edited keeps your edits, and one you deleted does not come back**. If saving fails (a read-only disk) nothing is
recorded and the next opening tries again.

**To get it back or reset it:**
* DASH > Dashboards > under the templates, **Local cluster (k3s)** makes a fresh copy (new id) next to yours. It needs a source of kind Kubernetes: if you
  deleted `kube-local`, add one (Sources > Add source > Kubernetes (local k3s), cluster `local`).
* To have the original recreated under its own id, delete `k3s-local` (and `kube-local`) if they exist and remove `"k3s-local"` from
  `<data dir>/dash/seeded-<user>.json` (or delete the file); it is seeded again the next time DASH opens. `seed::reset_default` does the same and also
  overwrites an existing `k3s-local` and `kube-local` with the shipped ones.

**Verified and not.** Tested with fake kubectl answers (`dash/kube_fixtures/*.json`, `dash/kube_tests.rs`): every view, the broken, empty, huge and odd
inputs, the cluster down, metrics missing, that only fixed `get`s are ever issued, the seeding rules. **Not verified on a real k3s**: the exact JSON this
k3s version prints, the real metrics-server answers and their timing, the kubelet volume statistics, the 20 s kubectl limit on a large cluster, and how the
20 panels look on a real window.

## Dashboards

`dash/model.rs`: `Dashboard { title, time {from,to}, refresh_s, default_source, variables, panels }`, panels on a 24-column grid
(`Grid {x,y,w,h}`), `Panel { title, source, query {text, target, step_s}, visual, options {unit, decimals, thresholds,
legend, min, max, legend_format, text, table settings} }`. Layout is repaired after every move so panels never overlap.
`$__from`, `$__to` (ms), `$__from_s`, `$__to_s`, `$__interval` (`30s`), `$__range`, `$name`, `${name}` and `[[name]]` are filled
in; several values of a variable are joined with `|`.

Dashboards are JSON files in `<home>/Dashboards/<id>.json`, written atomically; they hold source **ids**, never an address
or a credential. Import takes a path inside your folders (or the host files dialog), repairs the file and gives it a new id;
Export writes `Dashboards/exports/<name>.json` and, in window mode, offers to save it on the host computer
(`docs/HOST-FILES.md`). Templates: node_exporter, kube-state-metrics, Elasticsearch indices, ClickHouse `system.query_log`, DuckDB
file explorer.

## Several sources in one panel

A dashboard can use as many sources as it likes, and a **panel can mix them** (Grafana's "Mixed" style): Prometheus and
Elasticsearch and ClickHouse and DuckDB in one chart or table.

* **Queries A, B, C ... (up to 8).** A is the panel's `source` + `query` (so every older file loads as "query A" unchanged); B and
  the rest are `queries`. Each has its own source (empty means the dashboard's **default source**, set from the Sources screen),
  text, index/table/file, **alias** (legend, `{{label}}`), **unit**, **axis** (left or right: a second y scale with its own unit),
  a **hidden** switch (still available to expressions) and an optional timeout.
* **Parallel, with partial results.** Every query runs on its own thread with its own timeout and the cancel flag. If one source is
  down or the query is wrong, the others' data stays on screen and the failure is named at the bottom of the panel and with a red mark
  in its header ("B (Logs): connection refused ..."); only when every query fails is the whole panel an error.
* **One fetch for identical questions.** Answers are cached by (source, text, target, range, step, limit) in `multi::FetchCache`, shared
  by all panels and the assistant's previews; two panels (or two queries) asking the same thing make one request, even when asked at the
  same moment. A forced refresh forgets them; a failed or cancelled answer is never kept.
* **Expressions** (the Expressions tab, one line each, `dash/expr.rs`):

  | Line | Meaning |
  |---|---|
  | `C = A / B * 100` | math between series of different queries (`+ - * /`, numbers, parentheses, `rate(X)`, `abs(X)`); series pair by equal labels, by position, or one against many |
  | `D = merge A, B` | all the series of both |
  | `T = join A, B on key` | table join: rows with the same text in column `key` (ClickHouse rows with Elasticsearch counts) |
  | `E = top 5 A` | the five series with the highest average |
  | `F = filter A where host=web1` | keep series by label (`=` or `!=`) |
  | `G = group A by host sum` | sum, avg, max or min of series sharing a label, at identical times |
  | `R = rename A as "{{host}} cpu"` | a new legend from labels |
  | `align interpolate 30s` | how the other series is read at a point's time: `nearest`, `previous` or `interpolate`, within a tolerance (default twice its spacing); a point with no partner in range is dropped, and so is a division by zero |

  A line that fails is reported (`expression D: ...`) and the others still run. Results are drawn with the visible queries.
* **Template variables from any source.** A variable's values come from a query on any source (the first column: label values, SQL
  `SELECT DISTINCT`, an Elasticsearch `terms` aggregation). Each variable may have a **per-source mapping** (`{}` stands for the value,
  e.g. `{}.prod.example.com` for the Elasticsearch host field while Prometheus keeps the plain instance name), applied when a query of
  that source is built.
* **The editor** has a tab per query (with that query's source picker and the connector's own query box), a "+ query" tab, an
  Expressions tab, a row of the open query's settings (axis, shown/hidden, unit, "Preview this query", remove), a preview of the
  combined result and a preview of one query alone. Below 1000 px it is split into Setup, Query and Preview pages.
* **The assistant** passes `queries` and `expressions` to `dash_panel_add` and `dash_panel_update` (the expression lines are checked
  first; a failing source shows up as a note on the preview and as a badge, not as a refusal).

## Credentials are always in the vault

Checked end to end, each point with a test that plants a secret value and looks for it:

1. **A source stores only vault references** (`uvault:`, `vault:`, `ext:`), never a plaintext secret, in the source file, the dashboard
   JSON, exports, templates, imports, logs, the audit log, the assistant's context, tool arguments and results, or error messages.
   * A secret in a settings value (under any name), a non-reference in a secret slot, an address with a user and password
     (`https://user:pass@host`, also in the proxy) or a token in the query string (`api_key=`, `token=`, `access_token=`, `password=`,
     `x-api-key=`, `key=`, `sig=` ...) is **refused when the source is saved**, with a sentence that says to use the vault. A rejected
     value is never echoed back in the message.
   * Query texts, expressions, legends and notes of a dashboard are scanned with the chat's credential detector when a dashboard is
     saved, exported, imported or built by a tool; a credential found there is replaced by a placeholder.
2. **The add-source form has no plaintext secret field.** A secret slot is a list: "(none)", the names in your vault, and "(create a
   new vault entry...)". That opens a small dialog (name, secret value in a masked field); on Save the value is written straight to the
   vault (`Env::create_vault_entry`), the field is wiped, the source gets only `uvault:svc.<name>`, and only the name is audited. A locked
   vault gives "your vault is locked: unlock it (sign in again) first" and the typed text is wiped as well.
3. **Import and export.** Export writes a bundle of the dashboard and the sources it uses; sources go through the same cleaning as an
   import (addresses lose user, password and token parameters) and secret slots carry only the entry **names** as placeholders. Import
   strips plaintext, tells you what it removed, and keeps a placeholder only when your vault has an entry of that name; otherwise the slot is
   left empty and listed ("pick a vault entry for the Token"). Imported sources never overwrite one you have.
4. **Resolution per request.** The reference is read when a request is built, held in zeroising memory, and used only for the
   `Authorization` header (basic, bearer, `ApiKey`), or as the mutual-TLS key. Everything that comes back (response bodies, server error
   texts, transport errors) has every secret in use removed (`[redacted]`) before it is parsed, so a server that echoes the headers cannot
   put the credential into a frame, a table cell, an error or the assistant's reading; the credential detector is applied to error text
   as well. The assistant has no tool that reads or writes a vault entry, a source or a connector.
5. **Vault locked or entry missing**: each source says so on its own ("your vault is locked, so the token cannot be read" / "the vault
   entry `svc.prom` is missing"), with what to do, and never a value.

## The screen

Board, Dashboards (list and templates), Sources, Panel editor, Table view; the assistant pane beside them (a toggle that takes the
whole area when the window is under 1000 px wide, which is the 800x480 case). Mouse: click selects, drag a title bar moves,
drag a corner resizes, drag across a chart zooms the time range, click a legend entry hides that line, right-click for copy, edit
and "Ask AI about this moment". Keyboard: `F1` lists every key (arrows choose a panel, Enter edits, `a` adds, `v` table, `i` ask
AI, `Ctrl+Z` undo, `Ctrl+I` the assistant, `F6` moves the keyboard between board and assistant, `Ctrl+R` runs in the editor).
Questions to the sources run on worker threads (four at a time), results are cached per panel (a layout change does not
re-ask), the screen redraws only when data, the pointer over a chart, a spinner or the "updated Ns ago" text changes, and every
question is cancelled when the tile is left.

## The assistant

Ordinary chat (when the account may use DASH and the network): `dash_sources`, `dash_describe`, `dash_query`, `dash_explain`
(all Safe, read-only, capped) and `dash_panel_add` (Ask: previews the query, then saves the panel into a named dashboard).

Inside the DASH tile the chat is the same component as everywhere (`ChatApp::with_dash`; model picker, approvals, select and
copy, ask_user chips). It is given, with every message, a compact picture of the dashboard (`session::context_text`, at most 3,200
characters): title, range, variables, sources by name and kind, every panel with id, visual, source and the start of its query, the
selected panel and its result summary, recent errors. Its tools edit the **open** dashboard in memory:
`dash_panel_add` (runs the query once first and refuses a failing or writing one), `dash_panel_update`, `dash_panel_remove`,
`dash_set_range`, `dash_layout` are Safe (they are highlighted on screen and undoable with Ctrl+Z); `dash_save` asks. No tool
creates or changes a connector or a credential. The prompt rules tell it to call `dash_describe` before writing a query (no invented
names), to show the query it used, to answer "why" questions with `dash_explain` (numbers computed from the frame: min, max and
when, trend, peaks, biggest step, comparison with the previous window) and to link the panels it made. Chats are kept under their
own key, apart from the ordinary chat.

## How to add a connector

1. Create `crates/iyakki-agent/src/dash/<name>.rs`: a struct, `pub fn info() -> KindInfo` (label, language, fields via
   `net_fields(...)` for network ones), `new(cfg, deps)` (use `Http::from_config` so credentials, TLS, proxy and timeouts come
   for free), and `impl DataSource`. Check every text with `readonly.rs` before sending, and answer with a `Frame`.
2. Declare the module in `dash/mod.rs`, add the kind to `registry::kinds()` and `registry::build()`.
3. Test it with `connector::fake::FakeUp` (a scripted upstream) the way `prom.rs` does: a matrix or table, an error, odd JSON,
   writes refused, a secret only in a header. Add an `#[ignore]` live test to `dash/live_tests.rs`.
4. Add a template in `dash/templates.rs` if there is an obvious one. The add-source form, the editor's schema list, the
   assistant's tools and the screen pick the kind up from the registry; nothing else changes.

## Files

`crates/iyakki-agent/src/dash/`: `connector.rs` (seam, errors, HTTP), `frame.rs`, `readonly.rs`, `prom.rs`, `es.rs`,
`clickhouse.rs`, `duckdb.rs`, `neo4j.rs`, `kube.rs` (the local cluster, with `kube_fixtures/` and `kube_tests.rs`), `seed.rs` (default dashboards, once per administrator), `share.rs` (sharing, view only, with `share_tests.rs`), `registry.rs`, `model.rs`, `engine.rs`, `multi.rs` (queries in parallel, the shared cache, partial results), `expr.rs` (the expression stage), `templates.rs`, `store.rs`, `session.rs` (the open
dashboard, undo, the context), `explain.rs`, `tools.rs`. `crates/iyakki-shell/src/apps/`: `dash.rs` (the app),
`dash_charts.rs` (renderers and the maths under them), `dash_query.rs` (query box), `dash_tests.rs`.

## Not done

Alerting and annotations, share links, per-panel refresh, ES|QL, Prometheus `targets`, a linked DuckDB, other clusters than the local one, the other connectors
listed above, a remote backend (the seam is ready for it). Not verified here: a real server of each kind, a real DuckDB, a real
TLS server, bare metal, a real model.
