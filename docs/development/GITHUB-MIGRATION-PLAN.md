# Moving to GitHub as the primary repository (decided 2026-11-18)

**Decision (owner):** GitHub (`meisync/iyakki`) becomes the canonical, public repository so that other people can send pull requests. The old private history is NOT published (it holds personal emails and names, home paths, internal documents and unreviewed key-like strings). Gitea stays the private place for local parallel development.

## Shape
1. GitHub `main` starts with ONE clean root commit made from the alpha.6 tag by `dist/release/export-public.sh` (tracked files minus `dist/release/public-exclude.txt`, checks included), by the project identity, tagged `v0.1.0-alpha.6`. The earlier one-commit snapshots (alpha.2, alpha.3; alpha.4 and alpha.5 if the owner wants them) stay tags on their own branches.
2. From the root on, every commit is a normal commit under the project identity, never rewritten.
3. Gitea: the old repository becomes a read-only archive (`iyakki-archive`). Day-to-day work happens in a fresh Gitea repository cloned from the new `main`; agent lane branches and integration branches live there; work reaches GitHub `main` as ordinary commits after the scans (personal data, paths, tokens, licence headers). Pull requests merged on GitHub are fetched back into Gitea.
4. Internal process documents (`public-exclude.txt`) move to a separate private repository.

## Steps (after the alpha.6 tag; the alpha.7 kickoff)
1. Run `export-public.sh` on the tag; review its scan output and the personal-name scan; show the report to the owner. No push before the owner says go.
2. Replace the one-commit branches on GitHub or start `main` (owner's choice at that time); push `main` and the tag; GitHub release pages with the release notes.
3. Rebuild the working setup: new Gitea repo from `main`, lane worktrees and `mergeall` scripts re-pointed, the old repository archived.
4. Repository rules: CONTRIBUTING (exists), DCO sign-off check, branch protection on `main`, required review, CODEOWNERS, issue and PR templates, SECURITY.md (exists).
5. CI for pull requests: GitHub-hosted runners only, lint and fast tests; fork pull requests never run on our own hosts and never see secrets. The full gate (`release-green`, about an hour) runs on our hosts on a branch pushed after review.
6. Licence check for contributions (the base is permissive-only): dependency licence gate on pull requests, a note in CONTRIBUTING.
7. Progress page: wording changes to alpha.6 as the latest release.

## Not done
No history rewrite (decided against). The old commit hashes quoted in private notes stay valid only in the archive.
