# Decision needed: how a data import through the web service is approved (F-WEBSVC-08)

Status: waiting for the owner. Nothing is built. Written 2026-11-12.

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
