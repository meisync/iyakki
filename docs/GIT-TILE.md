# GIT: browse the repositories of your Git hosts

The **GIT** tile (Work group, subtitle *Browse repositories*) lets you look at the repositories of the Git hosts you connected
(Settings → Git, [`GIT-AND-SSH.md`](GIT-AND-SSH.md)) without cloning them: their files, commits and diffs, branches and tags, issues and
pull requests. It also lists the **clones already on this computer**. It is shaped like the K8S tile ([`K8S-TILE.md`](K8S-TILE.md)): the
connection at the top, a table below, a view of the chosen item, a status line, Refresh, retries and "the last list stays on screen".
**It never changes anything on a host**: every request to a host is a GET, and issues and pull requests are read-only.

## Who has it

Administrators, operators and users; not guests. It is remote-safe in Cloud mode ([`CLOUD-MODE.md`](CLOUD-MODE.md)): it needs no sound,
camera or display of its own. Light on memory (the same class as K8S).

## The top bar

* **Connection** (`c`): every connection of Settings → Git with a status dot (green after a good test, red after a failed one, grey not tested),
  its name, kind, address and where its data lives: *this computer*, *on the network*, *on the cluster* (the Gitea of your own cluster) or *on the
  internet*. The last row of the list is **Add connection...**, which opens Settings → Git with the wizard. **Local clones** is a pseudo-connection:
  the repositories on this computer, in the **Repositories** folder (`/home/<you>/Repositories`, else a folder under the data folder, which is where
  Clone puts them) and the ones the AI IDE's workspaces list. The choice is remembered.
* **Refresh** (`r`), **Show log** (`L`: the problems with the hosts, scrubbed, with Copy), **Add connection...** (`a`).
* With **no connection** the tile says *Connect a Git host to browse its repositories*, with the Add connection button and the hint for
  Gitea on your cluster (Settings → Dev cluster → Platform services sets it up; Add connection picks it up). The local clones are listed anyway.

## The list

A table: **name** (owner/repo), **description** (cut to the column; a tooltip shows all of it), **default branch**, **visibility**
(private/public), **last update** (age), **stars**, **size**. A green dot at the left says a clone of the repository is on this computer; it is
matched by the remote address (https, `git@host:`, `ssh://`, with or without `.git`, upper or lower case). Click a title or press `o` / `O` to sort,
`/` to filter by name, description or branch. A host with many repositories is read **50 at a time**: *Load more* (`m`) asks for the next page.
Enter opens a repository; narrow windows drop the least important columns and, on a phone-sized window, show one pane at a time with *Back*.

A list that cannot be read keeps the **last good list** on screen with a line over it. A host that is down or busy is tried again after 2, 4, 8, then 15
seconds (the ideas of the K8S tile's retry layer, `k8sretry.rs`, are shared: the `Tracker`, the backoff and the clock); a refusal (401, 404, an
untrusted certificate, a policy that forbids the host) is not retried and shows a calm panel with *Refresh*, *Show log* and *Edit the connection*.
A host that asks us to slow down (HTTP 429, or 403 with rate headers) shows **The host asked us to slow down: retry in n s** and everything
for that host waits; Refresh ends the wait.

## A repository

Tabs (`1`-`5`) and a branch or tag chooser (`g`):

* **Files**: a tree that loads **one folder at a time** (large repositories are never read whole; a folder shows at most 500 entries). A file opens in the
  viewer: text with line numbers up to 512 KB, the **README shown first** and Markdown rendered with the chat's Markdown layout (`v` or the *Source*
  button shows it as written), pictures (png, jpeg, gif, webp) scaled to the pane, and for a binary file or one over 512 KB its name, size and
  type only. `c` copies the text.
* **Commits**: the last 50 of the chosen branch or tag (short hash, subject, author, age). Enter shows the message and the **diff**: the changed files on
  the left (A/D/M/R, +added −removed), the lines coloured (+ green, − red, @@ cyan). A diff over 256 KB is not drawn; the file list stays and the
  text says to read it on the host's page.
* **Branches**: branches (the default marked) and tags; Enter switches the Files and Commits tabs to that ref.
* **Issues/PRs**: the open and closed issues, and pull requests or merge requests, with number, state, author, age and labels; Enter reads the text.
  Read-only: there is no new, comment, close or merge.
* **Info**: description, default branch, visibility, last push, stars, size, licence where the host gives it, where it comes from, the clone addresses
  (HTTPS and SSH) with **Copy**.

Actions (buttons and keys; each asks first where it changes something):

| Action | Key | What happens |
|---|---|---|
| Clone | `C` | A review (what, from which host, into which folder, how it signs in), then the AI IDE's clone job into the Repositories folder, with progress in the status line and the host-key card for a first-time SSH host |
| Open in AI IDE | `I` | With a clone: the folder is registered as a workspace (once) and the AI IDE opens it. Without one it offers *Clone* first and opens it when the clone is done |
| Fetch | `F` | The fetch job at once (it changes nothing of yours) |
| Pull | `P` | Looks at the clone first; if it has uncommitted changes a card says so, and the pull only fast-forwards |
| Push | `U` | Its own review: the branch, the origin and the host, how many commits ahead; nothing is forced or deleted |
| Copy clone URL | `y` | HTTPS or SSH, by the way the connection signs in |
| Open in browser | `w` | Shows the address and opens the BROWSER tile with it only after your yes |

No delete, rename or settings change of a repository exists in this tile.

## How the data is read

* **A host** (`iyakki_agent::gitbrowse`, through `GitHttp` of `gitconn.rs`): Gitea and Forgejo `/api/v1/repos/{o}/{r}/contents|commits|branches|tags|issues`
  (`/git/commits/{sha}.diff`, then `/commits/{sha}.diff`), GitHub `/repos/{o}/{r}/contents|commits|branches|tags|issues|pulls` (`Accept:
  application/vnd.github.diff` for a commit's diff), GitLab `/projects/{path}/repository/tree|files|commits|branches|tags`, `/issues`,
  `/merge_requests` (the diff comes as JSON and is turned into a unified diff). Files come as the host's base64 content, so a picture or a binary file
  is exact. The token comes from your vault through the connection and is sent only in a header from a private temporary file; the certificate you
  trusted for the host is used; the account's network policy is asked before every request; errors are scrubbed. A kind with no API (*Other git host*)
  can be cloned but not browsed.
* **A clone on this computer**: the git program through the runner, read-only (`ls-tree -l`, `show`, `log`, `diff` via `show --patch`, `for-each-ref`,
  `cat-file`, `status`, `remote get-url`); a picture is read through a temporary index and `checkout-index --prefix` into a private folder that is
  removed at once, so neither the working tree nor the index of the clone is touched.

Limits: 15 s per request; a page of repositories, commits or issues 50; a file 512 KB; a diff 256 KB; a folder 500 entries; a text file 50,000 lines.
Nothing is asked of a host while the tile is not on screen. Going back drops everything that was waiting to start; a request already on its way
finishes and is thrown away. Nothing blocks the screen: each request runs on its own thread and is picked up on the heartbeat.

## Safety

Read-only on hosts; a token is never shown, logged, put in an address, an audit line, a card or an error text. The audit records `git.browse.open`
(connection and repository name) when a repository is opened, and the clone, fetch, pull and push lines the AI IDE's jobs write
(`git.clone.start`, `git.fetch.start`, `git.pull.start`, `git.push`). The data of each view says where it comes from (the connection's place label).

## In the AI IDE (the Git tab)

The AI IDE has a **Git** tab after K8s (Ctrl+Shift+L, again to go back to the editor; the same tab strip and style as K8s). It is there for every account that may
open the GIT tile (all roles but guests); others see no tab. It has two sub-views that never mix, chosen with the two buttons at the top:

* **This project**: when the folder open in the IDE is a git working copy, the branch with ahead and behind and its upstream, the changed files (a letter
  for the status: M changed, A added, D deleted, R renamed, ? new and not tracked, U conflict; "staged" when staged), the recent commits, and a viewer for the diff of
  a changed file (against the last commit) or of a commit. Enter or a click shows the diff; `o` or **Open in editor** opens the file in the IDE editor (an
  untracked file has no diff and says so; a deleted file has nothing to open). Tab moves between the lists and the diff, `r` reads again, `c` goes to the
  connections. It is read with the git program (`status`, `log`, `diff`, `show`; `LocalRepo::working` and `diff_head` in `gitbrowse.rs`), on a thread, only
  while the tab is drawn and again every 5 seconds. A clean copy, a folder that is not a repository (how to start one) and a missing git program have their own words.
* **Connections**: this tile, drawn inside the tab: the connection picker (the list of Settings, Git; the last choice is remembered per person and shared with the tile),
  the repositories of the host, and a repository's files, commits, branches (default marked), tags, issues and diffs. Esc at the top returns to This project.
  With no connection it says *Connect a Git host...* and offers **Add connection...** (Settings, Git).

Read-only: **clone, fetch, pull, push and "open in the AI IDE" are refused in the tab** with the reason; they stay with the IDE's Repositories panel and Connect Git, which
ask first. With the Git tab open the chat gets a read-only context block (branch, ahead and behind, changed files by status and path, last commits, the file whose
diff is shown; never an address, a token or file text); the assistant uses the terminal tools it already has for anything else. No token is read or drawn by the working-copy view
(no credential is involved), and the Connections view uses the tile's own token handling.

## Connect Git or Gitea: a Gitea already on your cluster

When Add connection opens (and when you press R on the Gitea list) IYAKKI asks the dev cluster, read-only and in the background, for a ready Deployment that says Gitea. One that Platform services has no record of is listed as "Gitea on this computer (found on your cluster)" with its namespace; a record for the same namespace is shown instead, never both. If this computer has no address for it or IYAKKI has no token for it, the row says so in plain words and the one button is "Set it up (Quick install: Gitea)", which runs the existing Quick install item (it keeps the existing volume). When the cluster is not running or kubectl fails, nothing changes. Nothing secret is shown or logged; the audit line is `git.discover.cluster found <namespace>`.

A Platform services deploy over a Gitea that exists keeps how it is reachable (a ClusterIP Gitea stays ClusterIP) unless you choose another exposure. Git API calls, the connection test and the certificate fingerprint use IYAKKI's own HTTP(S) client; no `curl` is needed.

## Files and tests

`crates/iyakki-agent/src/gitbrowse.rs` (the readers, the diff parser, the clone finder; `gitbrowse_tests.rs`), `gitconn.rs` (the request now has an
Accept type, an answer limit and a time limit), `crates/iyakki-shell/src/apps/git_tile.rs`, `git_tile_repo.rs`, `git_tile_ui.rs`,
`git_tile_repo_ui.rs`, `git_tile_tests.rs`, `shell_git_tests.rs`, `crates/iyakki-shell/src/apps/ide_gitview.rs` (the AI IDE's Git tab; `ide_gitview_tests.rs`), and the two small `open_item` hooks in Settings (`git:add`) and the AI IDE (`ws:<id>`).
The tests use a scripted host (Gitea, GitHub and GitLab JSON) and a scripted git; one test clones a local repository with the real git and the real
jobs and browses it.

## Not done, said plainly

Nothing was tried against a real GitHub, GitLab, Gitea or Forgejo, with real repositories or on a real screen. The list cache lives while the tile is open
(it is not saved to disk). Issues and pull requests show the first 50 by update time, with no paging, comments or timeline. No search across repositories,
no blame, no file history, no compare. Pictures other than png, jpeg, gif and webp, and SVG, are listed as files, not drawn. A cancelled request cannot
stop a curl that already left; it finishes within 15 s and is discarded.
