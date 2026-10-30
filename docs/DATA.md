# DATA: tables, CSV and the data service

IYAKKI can open a table file, look at it, and answer questions about it, from the **DATA** tile (Work group), from the
assistant, and from any app or tile that embeds the viewer. All of them use one service, `iyakki-data`.

## The service (`crates/iyakki-data`)

A library with no UI. `DataService` is a trait; every call takes a plain request struct and returns a plain result, all of
them `serde` (JSON), addressed by a dataset **handle** (`d1`, `d2`, ...). That is what lets the service sit behind a remote
API later without changing a caller.

| Call | What it does |
|---|---|
| `open(OpenRequest)` | CSV, TSV (and JSON lines with the Polars engine): returns the handle, columns with types, row count |
| `schema(Pipeline)` | columns and row count after some steps |
| `page(PageRequest)` | rows `offset..offset+limit` of a pipeline, plus the total |
| `describe(Pipeline)` | per column: count, nulls, unique, min, max, mean, std, median, most common values |
| `materialize(Pipeline)` | keep a result as a dataset of its own (to join with or explore further) |
| `export_csv(ExportRequest)` | write a pipeline's result as CSV |
| `close(handle)` | forget it |

A **pipeline** is a handle plus steps (`Op`), which run in order: `filter`, `sort`, `select`, `rename`, `derive`,
`group_by` (count, sum, mean, min, max, unique, first, last), `join` (inner, left; on same-named columns) and `limit`.
In JSON: `{"handle":"d1","ops":[{"op":"filter","expr":"price > 10"},{"op":"sort","keys":[{"column":"price","dir":"desc"}]}],"offset":0,"limit":50}`.

Cell values are JSON: null, number, bool or string. Column types are `int`, `float`, `bool`, `str` (dates stay text).

### Expressions

Filters and derived columns use one small language, parsed and type-checked in the crate (so both engines agree and
mistakes come back as sentences that name the column or function):

```
price > 10 and contains(name, 'tea')       text in 'single' or "double" quotes, matching ignores case
`unit price` * qty                         columns with spaces: `backticks` or [brackets]
city in ('Paris', 'Rome') or x is null
round(price * 1.2, 2)                      + - * / %  = != < <= > >=  and or not
```
Functions: `contains starts_with ends_with lower upper trim len abs round floor ceil coalesce concat is_null`.
Comparisons with a missing value are unknown and drop the row; `and`/`or` follow SQL's three-valued logic.

### Two engines, one API

* **Built-in** (no dependencies): keeps only a table of where each row starts and loads a column into memory only when a
  filter, sort or statistic needs it. A file of a million rows is paged without being held in memory. CSV and TSV only.
* **Polars** (cargo feature `polars`, [pola.rs](https://pola.rs), MIT): the pipeline becomes a lazy query. The first
  2,000 rows are read straight from the file; going deeper parses the file once and keeps it. Adds JSON lines.

`iyakki_data::service()` returns Polars when the feature is on and the built-in engine otherwise; the desktop build turns
the feature on (`crates/iyakki-gui/Cargo.toml`), the Buildroot image does not. The service tests run against every engine
of the build, so they must agree.

## Where files may come from

The open dialog starts in, and stays inside, the person's **home folder** (`iyakki_agent::scope::home_root`: `/home/<user>`,
else `$HOME` in window mode, else a folder of the account under the data folder). The allowed folders, `FileRoots`, are that
home, the folders of the account's workspaces, any *narrow* folder the account may read (a scope like `/shared`; an
administrator's `/` does not count), and shared host folders (see `docs/HOST-FILES.md`). `..` stops at the top of a root,
links are resolved before checking so none can lead out, dot-folders are hidden (a toggle shows them), a typed path outside
the roots gets "… is outside your home folder", `~` means home, and `host:<name>/…` means a shared host folder. The dialog has
quick links (Home, Documents, Downloads, Workspaces, shared folders) and a list of recent files.

This is policy, not only the dialog: the viewer's open and the assistant's `data_open` / `data_export` use the same
`FileRoots` (read: inside the roots and allowed by the account's policy or a shared folder; write: as the policy says, and in
a shared host folder only when writing is on for it). The editor's Open dialog and the IDE's parent-folder chooser use the
same helper (the IDE keeps its workspace scope).

## The viewer (`crates/iyakki-shell/src/apps/data.rs`)

`DataView` is the component: `DataView::new(env, DataViewParams { path, filter })`, `draw(g, rect, scale, tick)` and
`handle(event)` (it says `Out::Close` when Esc has nothing left to dismiss). It carries its own file-scope checks
(`DataEnv`: the service plus read and write predicates), so a host only passes those in. `DataApp` is the tile around it.

* Virtual scrolling: only a window of rows around the visible ones is fetched, on a worker thread; the header is sticky.
* Click a header to sort (ascending, descending, off; Shift adds a second key). Type in the filter box and press Enter;
  a bad expression is explained under the box and the last good filter stays.
* Column width: `<` `>` narrow or widen, `=` fits, or drag the right edge of a header.
* Select with the mouse or Shift+arrows; `c` copies the cell, `r` the row, Ctrl+C the selection (tab-separated, what a
  spreadsheet pastes), Ctrl+A everything (up to 10,000 rows; the clipboard itself keeps 64 KB). The right-click menu has the
  same plus "Filter to this value" and sort.
* `s` shows the summary pane; `e` exports the current view (filter and sort applied) as CSV, inside the write scope.
* A `.csv` or `.tsv` opened in the AI IDE tree opens here; Back returns to the IDE.
* Window mode: **Host files…** (import, share a folder), **Save to host…** (the current view), and files dropped on the window.

## The assistant

Tools (offered to accounts that may use DATA and read files): `data_open`, `data_describe`, `data_query` (read, no
approval) and `data_export` (asks first). They take the structured steps above, never code. Answers are Markdown tables
capped at 50 rows and about 6,000 characters. Each person has their own service, so handles cannot be guessed across
accounts, at most eight stay open, and everything is dropped at sign-out. File contents are not written to the tool log.

Making and extending tables: `data_create` writes a new `.csv` or `.tsv` from columns (optionally with a type: text,
int, float, bool, date; cells are checked against it) and rows, into your home folder or a `folder` inside your file
scopes, and opens it (the call returns the handle to query next). At most 5,000 rows x 50 columns and about 2 MB go in one
call; more goes in batches with `data_append`, which adds rows to the file of an open table (or a `path`) and checks the
cells against the header, or places them by column name. Both ask first; the card names the file, the columns and the row
count, never the cells. An existing file is not replaced unless `overwrite` is true, a table in a read-only host folder is
refused, and files are written as a temporary file renamed into place, so one is never half-written. CSV quoting is done for
the assistant, and text that begins with `=`, `+`, `-` or `@` (a number such as `-5` stays a number) is stored with a leading
apostrophe so a spreadsheet does not run it as a formula. After an append the table is reopened and the call returns its
new handle. Editing and deleting rows are not offered. New files show in the Open dialog at once, and a table open in the
tab that changes on disk reloads within a few seconds (the filter is kept). The chat is told which table the DATA tab showed
last (file and column names only, never cells), so "this table" means that file.

## Cost of Polars

See the figures in the change report; in short, Polars is a build-time and size cost for the desktop only. The
Buildroot image keeps the built-in engine, so it does not grow.
