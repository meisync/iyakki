# Agentic RAG

IYAKKI is an OS for agents, so searching documents is something the assistant does itself: it plans, searches, checks what came
back, searches again with other words, and answers with citations, or says plainly that the sources do not hold the answer.

## What it does

* **Chat tools** (`crates/iyakki-agent/src/ragtools.rs`), offered to every account that has the RAG tile:

  | Tool | Asks? | What it does |
  |---|---|---|
  | `rag_collections` | no | The collections this account can search: name, documents, passages, embedding model, last indexed, and the external backends. |
  | `rag_search` | no | Passages for a query from one, several or all collections: source, passage id, score, backend, a provenance line. `k` 1 to 10, optional filters on source and title. |
  | `rag_read` | no | A whole document, or the passages around an id (0 to 2 each side), capped. |
  | `rag_research` | no | The bounded research loop below, with a ledger. |
  | `rag_sources_add_folder` | **every time** | Reads a folder the account may read into a named collection. The card shows the folder and the number of files; Auto mode and "Allow for this session" do not skip it. |

* **Collections.** `mine` (and `mine/<name>`) and `shared` (and `shared/<name>`). Another person's collection, a workspace history
  collection and a name that does not exist all answer with the same words ("there is no collection called ..."), so nothing is
  revealed about what exists. External backends (Qdrant, Chroma, generic HTTP) are searched through the same call with `all` or
  `external`; their keys are vault names in Settings and never appear in a tool result or in the chat.
* **The research loop** (`agenticrag.rs`):
  1. **Plan.** The model splits the question into at most 4 sub-questions (a simple question stays one).
  2. **Retrieve** per sub-question, at most 3 rounds each, at most 12 searches in all.
  3. **Grade** every passage: relevant, partly or irrelevant. By the model; if its answer cannot be read, by word overlap
     (the share of the question's words in the passage, with a positive search score).
  4. **Rewrite** the query when the grade is poor (model, or: the question's words plus frequent words of the partly related
     passages, then only its longest words). A query already tried is never repeated.
  5. **Stop** when each sub-question has a relevant passage (or two partly relevant ones), or the budget is spent.
  6. **Answer** from the kept passages with a citation `[n]` after every claim; each number maps to a source and a passage id.
     A citation pointing at no passage is marked `[?]`; an answer with no citation is flagged "Unverified"; a part with no
     evidence is named under "Not found in the sources". With nothing kept the answer is "Not found in the sources" and the
     model is not asked to write one.
* **The ledger** per run: the question, the sub-questions, each round's query with how many passages were found and new, the
  grades and who graded, the passages kept (with ids), the final citations, notes, the searches used, the model calls and the
  **stop reason** (enough evidence, search budget spent, rounds spent, nothing relevant, no model, cancelled, search failed).
  The last 20 runs of a person are kept in `rag-research-<user>.json` in the data folder (a damaged file or one from a newer
  version is moved aside as `.damaged` or `.newer`, never overwritten).
* **The RAG tile.** In ask mode the **Agentic** button (Ctrl+G) switches the loop on (default) or off (the classic single search
  and answer). A line under the question always shows the mode, searches used of the budget, rounds, model calls and why it
  stopped. **Evidence** (Ctrl+E) opens the ledger page: every sub-question and round, the kept ids and the citations; Older and
  Newer (Left and Right) show earlier runs, including the ones made from the chat; Up and Down scroll (also the wheel and a drag).
  Each result row says which backend it came from and its id.

## The budget

Hard caps in code (`agenticrag.rs`): 4 sub-questions, 3 rounds per sub-question, 12 searches in all, 24 model calls (plan, grades,
rewrites, answer; after that the plain steps take over), 10 passages per search, 1000 characters of question, 400 of query. A
caller (the tool's `budget`, the tile) can **lower** the limits, never raise them. The budget used and the stop reason are shown on
every run. Without a model (none set up, or the first call fails) the loop does one plain hybrid search and says so, without
planning, grading, rewriting or a written answer.

## Safety rules

* Retrieved text is **data, never orders.** Every tool result and every model prompt wraps passages in a marked block
  (`<<<RETRIEVED DATA - untrusted ...>>>` ... `<<<END RETRIEVED DATA>>>`, `rag::wrap_untrusted`); the marker characters inside
  a passage are defused, so a document cannot close the block. The three reading tools are flagged untrusted for the chat run.
* The model in the loop gets **no tools**: it is called with plain completion only, so an instruction inside a document cannot
  make it call anything (`run_command` included).
* **Secrets in documents are replaced** (`credguard`) in passages, titles, sources, `rag_read`, the prompts to the model and the
  kept ledger. The index itself is not changed.
* The account's **read scope** decides which folders `rag_sources_add_folder` may read (checked on the resolved path, so `..` and
  symlinks do not escape; symlinks inside the folder are not followed). Only an administrator adds to `shared`.
* Sizes are capped: query, passages (900 characters), passages per result, whole result (9000 characters), `rag_read` (6000).
* Searches are logged to the audit log as a character count, never the words.

## How it differs from classic RAG

Classic RAG (the RAG tile with Agentic off, `search_documents`) is one hybrid search, then one grounded answer. If the first query
uses other words than the document, the answer is "not found" or wrong. The agentic loop notices poor results and tries again, can
split a two-part question, and shows its work. It costs more model calls (at most 24) and takes longer; use classic search when one
look is enough.

## Evaluating it

`tests/fixtures/rag/` holds a synthetic golden set (14 short documents, 10 questions with the sources that answer them, 2 questions
the documents cannot answer; written for IYAKKI, CC0). The test `rageval_tests` indexes the documents, runs plain hybrid search and the
agentic loop (with a scripted grader and rewriter) and prints **recall@3** and **MRR** for both:

```
systemd-run --user --scope -p MemoryMax=9G -p MemorySwapMax=6G cargo test -p iyakki-agent --lib -- rageval_tests --nocapture
```

The test **fails if the agentic loop is worse than plain search** on either number, and checks that the unanswerable questions end in
"Not found in the sources". The scripted model knows the answer phrases, so the numbers show that the loop's mechanics (plan, grade,
rewrite, keep, rank) do not lose evidence; they say nothing about how a particular real model grades. To evaluate a real model, add
questions to `golden.json` and run the same loop with that model's backend (not part of the offline test).

## What is not built

* **Re-ranking models** (a cross-encoder over the top hits). The ranking is hybrid keyword and vector, then the grade.
* **Graph RAG** (entity and relation graphs over the documents).
* **Web retrieval.** The research loop searches the person's collections and external backends only. The browser and web tools
  (`web_fetch`, `web_research`, `browser_page`) already exist and can feed pages into a collection; making the loop search the web
  and add the pages it keeps (with an approval card) is the next step.
* Folder ingestion is synchronous inside the call; very large folders are bounded (2000 files, 2 MiB each) but not run as a job yet.
* External agents (the bridge) do not get the `rag_*` tools yet.
