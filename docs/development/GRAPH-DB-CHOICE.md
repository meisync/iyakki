# Graph database for the AI: the choice

Owner request: "quick install for better local graph db to use with ai". This file records how the database of the Quick install item
**Graph database (for AI)** (Store -> Setup) was chosen, and the design for using it from the AI IDE (section "Use by the AI IDE";
not built yet). Everything below was checked on 2026-10-26 against the upstream repositories, Docker Hub's registry API and by running
the images on a development machine; what could not be checked is marked "not verified".

## The choice

**Neo4j Community Edition 2026.10.1**, as its own pod in the managed k3s.

* Image: `docker.io/library/neo4j:2026.10.1-community`, pinned by digest (one constant, `GRAPH_IMAGE` in `quickinstall.rs`):
  index `sha256:f238979f3c80da6f383f2e4fe91be4dc3580d7ee3b7edec83d0b8783c53b74a4`
  (amd64 `sha256:1fe282e747ecdd61664801935a843cd4d20ec5ff9693a4fa7aca1fb34e245e6a`,
  arm64 `sha256:e4d20085a77816a4fbf7f10e30be80d92f3cf160dde8dbe417520ee3e11032b8`), pushed 2026-10-23.
* Licence: **GPL-3.0**, read from the image's own `/var/lib/neo4j/LICENSE.txt` and the upstream `LICENSE.txt` (a notice says the
  software is "licensed under the GNU GENERAL PUBLIC LICENSE Version 3 to all third parties", with commercial agreements possible on
  top). OSI approved, strong copyleft. It runs as a separate pod that IYAKKI talks to over HTTP, and nothing of it is linked into or
  shipped with IYAKKI; the cluster pulls the image from Docker Hub at the click.
* Why it wins although it is the heaviest of the three real candidates: it is the only one that satisfies the safety rule of the
  request (reads only, enforced by the server) over plain HTTP, which is what IYAKKI's existing HTTP client, vault and `cluster://`
  code speak, so no wire-protocol client had to be written. See "Why not the lighter ones".

Honest cost: about **490 MiB** of memory when idle (measured, heap 256 MB, page cache 64 MB, the JVM and metaspace on top). The item is
marked as needing about 3 GB of machine memory and the pod is limited to 1 GiB. A laptop with 4 GB or less should use another item.

### Why not the lighter ones

The priority list puts memory first. Two candidates are about ten times lighter, and were rejected for reasons that are not about taste:

* **Memgraph Community 3.14.0** (47 MiB idle, non-root user `memgraph`, openCypher, vector index worked): **no HTTP query API** (the
  image exposes only Bolt, 7687), and the Community edition **cannot make a read-only user**: `GRANT MATCH TO viewer` answered
  "Access to advanced authentication features requires an enterprise, ai_platform, or oem license", so every account can write. Licence
  BSL 1.1 (not OSI; internal use allowed, no database-as-a-service, converts to Apache-2.0 on 2030). It remains the runner-up and the
  right choice for a later "light" variant if an in-house Bolt client is written.
* **FalkorDB 6.0.3** (68 MiB idle, openCypher, vector index worked, `GRAPH.RO_QUERY` refuses writes, Redis ACL works): **SSPL v1**
  (not OSI), the image **runs as root** (uid 0 verified), and it speaks the Redis protocol, not HTTP.

## Comparison

"RAM" is measured idle for the three that were run (this machine, amd64); the others are not measured. "Query" and "vector" for
projects I did not run are from their documentation or from memory and are marked "(not verified)".

| Database | Licence (upstream file) | OSI | Query language | Vector index | Official image, version verified | Digest (index) | amd64 / arm64 | Non-root | RAM idle | Ports | Read-only | Maintained (last release) |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **Neo4j Community** | GPL-3.0 (`LICENSE.txt`, image) | yes | Cypher (openCypher superset), GQL-aligned | yes, created in Community (tested) | `neo4j:2026.10.1-community` (also `5.26.32-community` LTS) | `sha256:f238979f...53b74a4` | both | yes with `runAsUser` 7474 (entrypoint starts as root, works with `--user`, read-only root fs, all caps dropped, tested) | 489 MiB (256 MB heap) | HTTP 7474 (Query API v2), Bolt 7687 | `accessMode: Read` per request, writes refused by the server (tested) | 2026-10-23 |
| Memgraph Community | BSL 1.1 (`licenses/BSL.txt`) | no | openCypher | yes (tested) | `memgraph/memgraph:3.14.0` | `sha256:a7324212...cc05e` | both | yes (uid 101) | 47 MiB | Bolt 7687 only | no: needs Enterprise (tested) | 2026-10-21 |
| FalkorDB | SSPL v1 (`LICENSE`) | no | openCypher | yes (tested) | `falkordb/falkordb:6.0.3` | `sha256:0fb85036...e0812` | both | no, root (tested) | 68 MiB | RESP 6379, browser 3000 | `GRAPH.RO_QUERY`, ACL (tested) | 2026-10-22 |
| Kuzu | MIT | yes | Cypher | yes (not verified) | none official (embedded library) | - | - | - | - | - | - | **archived** 2025-10-10, last release v0.11.3 |
| Apache AGE | Apache-2.0 | yes | openCypher inside PostgreSQL | via pgvector, a separate extension (not verified) | `apache/age:release_PG17_1.6.0` (newer tags PG17_1.7.0, PG18_1.8.0 exist) | `sha256:fe8b3390...aa5be` (1.6.0) | amd64 checked | postgres image conventions (not verified) | not run | 5432 | PostgreSQL roles | 1.8.0-rc0 2026-07-09 |
| ArangoDB | BSL 1.1, change date 4 years, Apache-2.0 after (`LICENSE`) | no | AQL (not Cypher) | not verified | `arangodb:3.12.12` | `sha256:ae1ce5af...c88c4aa` | both | not set in the image | not run | 8529 | - | 2026-10-22 |
| SurrealDB | BSL 1.1 (`LICENSE`, no database-as-a-service) | no | SurrealQL (not Cypher) | yes (not verified) | `surrealdb/surrealdb:v3.2.6` | `sha256:5deaf059...bb9151` | both | yes (`nonroot`) | not run | 8000 | - | v3.3.4 2026-10-19 |
| Dgraph | Apache-2.0 (`LICENSE.txt`) | yes | DQL and GraphQL (not Cypher) | not verified | `dgraph/dgraph:v25.5.1` | `sha256:f7459f39...a38becb` | both | not set in the image | not run | 8080, 9080 | - | 2026-10-22 |
| NebulaGraph | Apache-2.0 | yes | nGQL (own), openCypher subset | no (not verified) | `vesoft/nebula-graphd:v3.8.0` and two more services | `sha256:1040573c...c74e05` | both | not set | not run | 9669 | - | v3.8.0 2024-05-17 (stale) |
| TypeDB | MPL-2.0 (`LICENSE`) | yes | TypeQL (not Cypher) | not verified | `typedb/typedb:3.14.1` | `sha256:d39d4f29...bb680` | both | not set | not run | 1729, 8000 | - | 2026-10-22 |
| JanusGraph | Apache-2.0 (`LICENSE.txt`) | yes | Gremlin (not Cypher) | via an index backend (not verified) | `janusgraph/janusgraph:1.1.0` | `sha256:75f57aff...c96d4` | both | yes (`janusgraph`) | not run, JVM plus a storage backend | 8182 | - | v1.1.0 2024-11-09 (1.2.0 not released) |

Version and last-release dates are from the GitHub releases API and Docker Hub tags API of that day (nothing guessed). MCP servers and
client libraries: Neo4j has an official MCP server and drivers for every major language (not verified here, and IYAKKI does not use
them: it speaks the HTTP Query API itself); Memgraph and FalkorDB have MCP servers and Redis/Bolt clients (not verified).

### How the priorities came out

1. Little RAM: Memgraph (47) and FalkorDB (68) beat Neo4j (489). **Neo4j loses this priority** and is chosen anyway, see above.
2. Licence for a click-download: all three are acceptable as a separate pod; Neo4j is the only OSI one (GPL-3.0). The Quick install text
   says plainly: free to run yourself, GPL copyleft applies to Neo4j itself, not to IYAKKI; commercial editions exist and are not used.
3. Cypher and vectors: Neo4j, Memgraph, FalkorDB, Kuzu, AGE (openCypher). The rest use another language.
4. Maintained: Kuzu is archived; Nebula and JanusGraph have had no release since 2024.
5. Non-root official image: Neo4j (`runAsUser`), Memgraph, SurrealDB, JanusGraph; not FalkorDB.

Rule 1 of the project (build in-house first) does not apply: a graph database is not something to rewrite. The in-house part is the
small connector (`dash/neo4j.rs`, about 300 lines on the existing HTTP client).

## How it is deployed

Namespace `iyakki-data` (with Elasticsearch), a 5 Gi PVC, one Deployment (Recreate, one replica), a ClusterIP Service on 7474 (HTTP) and
7687 (Bolt, for programs of your own in the cluster). Pod: non-root (uid/gid 7474), root file system read-only (tmpfs-like
`emptyDir`s for `/tmp`, `conf`, `run`), all capabilities dropped, no privilege escalation, no hostPath, no service account token,
memory request 640 Mi / limit 1 Gi, heap 256 MB, page cache 64 MB, telemetry off. The administrator password is generated at the
click (24 random characters from `iyakki_sec::random_fill`), put in a Kubernetes Secret created with the managed `kubectl` from a
0600 file that is deleted at once, and handed to the person's vault (`uvault:svc.graphdb`) by the shell; it is never in a manifest,
a log, a job line, the audit log or a test.

DASH gets a source "Cluster graph database" of the new kind `neo4j` at `cluster://iyakki-data/graphdb:7474` with basic sign-in from that
vault entry. The assistant then reaches it through the DASH tools it already has (`dash_sources`, `dash_describe`, `dash_query`): the
kind sends every query through the Query API with `accessMode: Read` (the server refuses writes), a row cap, a timeout, plus a text
check that refuses write words and procedure calls before anything is sent. No separate `graph_query` tool was added: it would have
duplicated `dash_query` (same caps, same redaction, same cluster addressing).

## Use by the AI IDE

Goal: the IDE agent asks a code graph for symbols, definitions, callers, callees, imports and file relations instead of reading whole
files, so a chat turn sends fewer tokens. **Design only; no indexer is built.**

### 1. Code graph and indexer

What the tree has: `crates/iyakki-edit/src/syntax.rs` is a hand-written line highlighter (token kinds `Function`, `Type`, `Keyword`,
`Macro` per language, `lang_for_path`), not a parser; there is no outline, symbol or LSP code anywhere (`ide*.rs`, `iyakki-edit`,
`iyakki-agent` were searched), and **tree-sitter is not in the tree** (`Cargo.lock` has no entry). Tree-sitter itself is MIT, but each
language grammar has its own licence and every grammar is a C library compiled in: it needs the permissive-licence check and the
"build in-house first" decision of the owner before it is added; it is not assumed here.

Plan, in-house first:

* **Schema** (labels and relationship types; every node carries `ws` = the workspace id):
  `(:File {path, lang, hash, lines, indexed_at})`, `(:Symbol {id, kind, name, qname, line, end_line, col, sig})` with `kind` in
  `fn|method|struct|enum|trait|class|const|module|test`, `(:Module {name})`.
  Edges: `(File)-[:DEFINES]->(Symbol)`, `(Symbol)-[:CALLS]->(Symbol)`, `(File)-[:IMPORTS]->(Module|File)`,
  `(Symbol)-[:IMPLEMENTS]->(Symbol)`, `(Symbol)-[:TESTS]->(Symbol)`, `(Symbol)-[:REFERENCES]->(Symbol)`.
  Uniqueness constraints on `(ws, File.path)` and `(ws, Symbol.id)`. Calls and references that cannot be resolved to one definition
  are stored on a placeholder `Symbol {kind:'unresolved', name}` so a name search still finds them (marked low confidence).
* **Indexer, level 1 (all languages, in-house):** a line scanner over `syntax::highlight_line` tokens per language gives definitions
  (`fn`, `struct`, `class`, `def`, `function`... by keyword followed by a name) with line ranges by brace/indent depth, imports by
  `use`/`import`/`require`/`#include` lines, and calls/references by name matches of known symbols (identifier followed by `(`).
  Honest limits: no types, no overload or macro resolution; the answer says "by name".
* **Level 2 (optional, per language):** a real parser. Rust through the `syn` crate (MIT or Apache-2.0, already a build-time
  dependency of most Rust trees: check `Cargo.lock`) is the first candidate, as the repository is mostly Rust; tree-sitter only if the
  owner approves the grammars' licences.
* **Incremental:** on file save (the IDE already has a save event) the content hash (SHA-256 of the bytes, `iyakki_sec`) is compared
  with `File.hash`; only changed files are re-parsed: delete that file's `DEFINES` subtree and its outgoing edges, insert the new
  ones in one transaction. A first full index walks the workspace honouring `.gitignore`, skips binaries and files over 1 MB.
  A background thread does it in batches of 100 statements, never blocking the UI.
* **Per workspace and per person:** one Neo4j database per workspace is not possible in Community (one user database), so every node
  has `ws = "<person>:<workspace id>"` and every generated query starts with `MATCH ... WHERE x.ws = $ws`; the parameter is set by the
  tool from the signed-in account, never by the model. Two accounts never share a `ws`. Deleting a workspace deletes its `ws` nodes.
* **Size caps:** 200 000 nodes and 1 000 000 edges per workspace, 2 000 symbols per file; beyond that the index stops and says so.
  The PVC (5 Gi) is checked before a full index.

### 2. IDE agent tools (read-only, new, later)

`code_symbols(query)`, `code_definition(symbol)`, `code_callers(symbol, depth)`, `code_callees(symbol, depth)`, `code_outline(file)`,
`code_impact(file | symbol)`. The model passes names and numbers only; the tool builds fixed Cypher with `$ws` and bound parameters
(no raw Cypher from the model; if one is ever allowed it goes through the same `accessMode: Read` and text check as `dash_query`).
Results are short and capped (50 rows, depth at most 3, 4 KB of text), each row `path:line-end_line  kind name  (confidence)`, so the
model then calls `read_file` with a line range for just the code it needs. The IDE system prompt gets one paragraph: "for finding
where something is defined or used, call the code_* tools first and read only the returned ranges; fall back to grep and read_file
when a code_* tool says the index is missing or stale". Every `code_*` call is a `Risk::Safe` tool (reads a local database, no side
effects), listed in the audit log by name and parameters only.

### 3. Measuring the token saving

The ledger exists: `crates/iyakki-agent/src/usage.rs` (`UsageLog::record_call(Call { src, cat, input, output, ... })`, read by the
USAGE tile and `totals_of`). Add one category label per IDE turn, `cat = "ide:graph"` when the code_* tools were offered and
`cat = "ide:plain"` when they were not, set from a per-chat flag; `by_source`/`totals_of(user, "assistant", Some(cat), from)` then
give tokens in and out per arm. Benchmark on this repository: a fixed set of 20 tasks ("where is `X` defined", "who calls `Y`", "what
breaks if `Z` changes", "list the functions of this file", "which tests cover `W`"), each run once with the tools and once without, in
alternating order, same model, temperature 0; record tokens in, tokens out, tool calls, and whether the answer was right (a person
checks against `git grep`). Report the median saving per task type; the aim is a cut of input tokens of at least 40 percent on the
navigation tasks with no loss of correctness, otherwise the feature is not worth its memory.

### 4. Fallbacks, privacy, security

* Database not installed or not Ready: the tool answers "the code graph is not installed (Store -> Setup -> Graph database)" and the
  agent falls back to grep and outline; the IDE shows the same line once with a button to Setup. Index stale (a file hash differs or
  the index is older than the last save): the tool says "index is behind for N files" and returns results marked stale, or refuses and
  names the files to read directly.
* Privacy: the graph lives on the local cluster's volume; the only network path is cluster IP to the DASH connector; no code, symbol
  or path is sent anywhere but to the model the person already chose, and then only as the short results above.
* Security: queries are generated by the tool from fixed templates; the connector uses `accessMode: Read`; the indexer is the only
  writer and runs in the shell with the admin credential from the vault (the model never sees it); the `ws` filter is applied by code;
  results are treated as data, never as instructions (file names and symbol names come from the repository).

### 5. Steps, sizes, risks

1. Schema and writer over the Query API (write path with `accessMode: Write`, batched, in a new `codegraph.rs`): about 600 lines.
2. Level-1 scanner for Rust, Python, JS/TS, C: about 800 lines; tests with fixture files.
3. Incremental update on save, workspace walk, caps: about 300 lines.
4. The six tools and the prompt paragraph: about 400 lines.
5. A/B counter and the benchmark script: about 150 lines.
Risks: name-based call edges are wrong for common names (mitigated by confidence and by always showing `path:line` to verify);
Neo4j's 490 MiB on small machines (mitigated by the Setup warning, and by the Memgraph runner-up behind the same connector idea);
index drift (mitigated by hashes and the stale message); GPL-3.0 of the server (separate pod, HTTP only).

## Not verified

Anything marked "(not verified)" above; Neo4j Community under the real k3s (read-only root file system was tested with Docker, not
Kubernetes); arm64 images were checked on the registry (manifests, sizes, user) but not run; the idle memory of the candidates that
were not run; the licence of every Neo4j third-party library (the image carries `ThirdPartyLicenses.txt`; not read in full).
