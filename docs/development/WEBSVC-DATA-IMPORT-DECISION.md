# Decision needed: how a data import through the web service is approved (F-WEBSVC-08)

Status: option A chosen by the owner (2026-11-16) and built. Written 2026-11-12.

## What was built (2026-11-16)

`POST /api/v1/data/call` with `data_create`, `data_append` or `data_export` (cloud profile only) answers 202 with an import id and a plain summary (counts and names, never a cell); the rows are kept in memory only and nothing is written. The pending import shows in `GET /api/v1/approvals` (kind `tool_call`, run `data`), and the person approves with `POST /api/v1/approvals/data/{id}` from a DIFFERENT sign-in than the one that asked (the asking token is refused with 403, so a stolen script token cannot approve itself). It expires after five minutes, at most five wait per person, an approval runs the tool once through the person's own policy, and the requester reads the outcome once with `GET /api/v1/data/imports/{id}`. Tested through the real routes on the web service's fakes (`webservice::tests::a_data_import_waits_for_an_approval_from_another_sign_in_and_runs_once`, `webservice::dataimport::tests`); not run on a real cloud machine, and the approval page row (a button in the browser's approvals list) is the existing list. Open: option B (INBOX card on a desktop) was not added.

## The question

`POST /api/v1/data/call` offers only the read tools (`data_open`, `data_describe`, `data_query`). Making it also run `data_create`, `data_append` and `data_export` writes files in the person's data jail, so it needs the person's approval. The difficulty: a script or program that calls the API holds a bearer token, and an approval sent with the same token proves nothing (a stolen token would approve itself). The approval has to come from something the token holder does not control.

What stays the same in all three options: the request is stored as pending with a plain summary (tool, table or file name, number of rows or bytes, never the data), expires after 5 minutes, runs once, is audited by name and counts, and the data is read only after the approval (so a refused import leaves nothing behind but the pending note).

## Option A: approve on the web page (the Approvals page)

The pending import appears in the person's list of approvals (`GET /api/v1/approvals`, the same list a risky tool uses) and the person clicks Approve in a browser session.

* For: one place for every approval; works on a cloud pod with no desktop; the browser session is a separate sign-in from the script's token.
* Against: needs a person with a browser open; the script waits (it polls or gets a callback); a browser session of the same person with the same rights can be driven by the same attacker if the machine is already taken over.
* Work: the pending store for non-run approvals, the page row, the API `POST /api/v1/approvals/data/{id}`; about two days.

## Option B: approve in the INBOX on the desktop

The request becomes an INBOX approval card on the machine (the one a flow's Approval step uses). Works only where a desktop exists.

* For: the strongest separation (a different device and a different channel from the script); reuses the card.
* Against: a cloud pod has no desktop, which is the case this row is for; a person away from the machine cannot approve.
* Work: small if desktop only; for the cloud it needs the INBOX to be reachable from the web page, which makes it option A.

## Option C: fresh password re-entry in the same call

The write request carries `confirm_password`; the service checks it against the account and runs at once. No pending state.

* For: simple, synchronous, no second device; a stolen token alone cannot import.
* Against: the password then passes through scripts and their logs and shell histories, which the project avoids ("secrets: never in arguments, logs, tests"); it trains people to put passwords in programs; no per-import summary is shown to the person before they commit.
* Work: about half a day, but it conflicts with the project's secret rules.

## Recommendation

Option A for the cloud profile (it is the profile the row is for), with option B added later for desktops that want it. Option C is not recommended. If the owner agrees to A, the next step is the pending store and the page row; the three write tools then go through the same `data_call` path with `needs: Data` and `audit: true`.
