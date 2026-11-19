# Notifications for cloud mode: audit, design, threat model

Status: built 2026-11-04 (hub, channels, web routes, event stream, bell and page). Owner questions: "how notifications work", then
"notifications and approvals can be triggered by workflows", then "the user session may go away, but scheduled workflows should keep
running; notifications and approvals can be received".

Marks: **[proven]** = read in the code or run in a test on 2026-11-04. **[guess]** = taken from the owner's description or from a
feature row, not re-read in code today.

## 1. Audit: what existed

| Piece | Where | Mark |
|---|---|---|
| Per-person INBOX store: kinds (info, warning, error, reminder, agent, system, device, download), `Link {app, id}`, levels, priority, auto-grouping, retention, snooze, flagged, `needs_attention`, 300 notes, one JSON file per person, atomic write, mode 0600, damaged file moved aside | `crates/iyakki-agent/src/inbox.rs` | proven |
| `Inbox::new(dir)`: the directory is a parameter, so a tenant volume path works without change | inbox.rs | proven |
| Counts: unread (not snoozed), attention, other, flagged, open tasks, overdue | `Inbox::counts` | proven |
| Bell with count (unread + approvals waiting + overdue tasks, red while an approval waits), Notifications page, tasks, sounds, tools `notify`, `task_add`, `task_list`, `task_update` | FEATURES F-INBOX-01..03 | guess (rows read, shell code not re-read) |
| Tool-call approvals: only the run's owner answers, one call at a time | `webservice/handlers.rs` (`approvals_list`, `approvals_decide`), `runs.rs` | proven |
| Tenant approvals: router (role holders, team admins, then tenant admins, never the requester), pending list, approve/deny, audit, expiry (24 h default) | `tenantconfig/approvals.rs` | proven |
| Flow `notify` node: writes the INBOX note (`Host::notify`), then channels; only `matrix` is wired, any other channel answers "the channel `x` is not available" | `flows/notify_matrix.rs`, `iyakki-flow/src/host.rs` (`Notice`, `ChannelNotice`) | proven |
| Flow `approval` node | docs/FLOWS.md | guess |
| Flow webhook gate: HMAC-SHA256 over `<ts>.<body>`, headers `X-Iyakki-Timestamp`/`X-Iyakki-Signature`, 5 minute window, replay memory | `flows/webhook.rs` | proven |
| Web API for the inbox | `webservice/routes.rs` | proven absent |
| Live event stream for the web UI | `webservice/server.rs`, `app.js` (TODO comment about it) | proven absent |
| Delivery outside the page (email, webhook, push) | whole tree | proven absent |
| Unified approvals view | `GET /api/v1/approvals` listed tool calls only | proven absent |
| Routing rules (escalation, quiet hours, digest) | whole tree | proven absent |
| A cloud-flows `NotifySink` (`cloudflows/notify.rs`) | not in the tree when this was built | proven absent |

The consequence for sessions: the INBOX was already durable (a file), but nothing wrote to it unless a person's shell was running, and
tenant approvals produced no notification at all. Both change here.

## 2. Design

The hub lives in `crates/iyakki-agent/src/notifyhub/`. All of its state is on the tenant volume under `cloud/notify/`:
`inbox/<person>.json` (the existing store), `hub.json` (the state file: fan-out jobs, delivery queue, dead letters, dedupe memory,
escalations, digest items, rate counters, opt-outs, approval tracking) and `lease.json`.

```
producer (flow node, scheduler, approvals worker)
   |  emit(Notification)               NotifySink::emit
   v
1. dedupe check + write a fan-out JOB to hub.json          <- durable from here on
2. expand recipients (people | roles | team) against the tenant's roles
3. INBOX note per person (redacted), event to the person's open sockets
4. in ONE step: channel deliveries (routing rules, rate limits, opt-outs),
   an escalation record, and removal of the job
   v
worker tick (leader only): jobs left by a crash -> approval reminders -> escalation -> digest -> due deliveries
```

* **Durability and sessions.** Nothing depends on a signed-in person. A crash after step 1 is repaired by the next tick (the job is
  re-run; INBOX notes already written are remembered per person). Unread counts are read from the INBOX files, never from memory
  (`notifyhub::api::count`), so a bell shows the truth when a person signs in days later.
* **The worker** is `Hub::tick(now, net)`. It takes a lease (`lease.rs`, file lock, 60 s TTL, renewed by the holder) so only one
  replica delivers; any replica may emit. The cloud scheduler's lease helper did not exist when this was built; `lease.rs` is small and
  can be replaced by it. The service process must call `tick` about every 10 s (see section 8).
* **Contract.** `sink::NotifySink::emit(&Notification) -> Result<Receipt, String>`; `Notification {tenant, team, recipients, severity,
  title, body, link {app, id}, dedupe_key, source}`; `Recipients = People | Roles | Team`; roles are `TenantAdmin`, `TeamAdmin`,
  `Approver(scope)`, `Group(name)`. The hub refuses a notification whose `tenant` is not its own.
* **Severity** `low < normal < high < critical`. A channel takes everything at or above its `min_severity`. Critical goes through quiet
  hours. Low goes to the daily digest where one is configured.
* **Routing rules** are pure data in `notify.routing` (tenant or team): `quiet_start`, `quiet_end`, `utc_offset_minutes` (a fixed
  offset is enough), `digest_time`, `escalate_after_minutes`, `escalate_min_severity`. Pure code in `rules.rs`, tested on a fake clock.
* **Escalation** (`escalation.rs`): no acknowledgement within T moves a notification up: the people it was for, then the team's admins
  ("Not acknowledged: ..."), then the tenant's admins, then it ends. Any recipient's `ack` (which also marks the note read) stops it.
* **Approval reminders** (`reminders.rs`): the worker reads the tenant approvals on file and, for each pending one, notifies the eligible
  approvers once when it appears, at 50 % and 90 % of its time to live, and the approvers and the requester once when it expires. The
  keys are persisted, so a restart repeats nothing. Any eligible approver can decide at any time before expiry from any session with
  their own token (`/api/v1/tenant/approvals/{id}/approve|deny`); the second decision is refused with 409 "already decided".
  There is no signed link that decides: **an approval always needs an authenticated click in the UI.**
* **Queue** (`queue.rs`): persistent, at most 2000 entries, bodies cut at 8 KiB, backoff 30 s doubling to 1 h, a dead letter after 6
  tries (the last 200 are kept). A switched-off or removed channel drops its queued deliveries without sending.
* **Rate limits and dedupe**: per channel (`max_per_hour`, default 60), per recipient (20 an hour), dedupe key (one hour, remembered
  across restarts). Over the limit the delivery is dropped and counted; the INBOX note is never limited. Per-person opt-out per channel
  (`Hub::set_optout`; no route or page yet).
* **Event stream** `GET /api/v1/events` (WebSocket, `webservice/events.rs`): the first client message is
  `{"type":"auth","token":...}` within 10 s (the token is never in the address); then only that person's `ready`, `notification`,
  `count` and `approval` events; a ping every 25 s; a silent client is dropped after 90 s; a slow client loses its OLDEST events
  (queue of 100); at most 5 connections per person and 200 per tenant. The client cannot ask for anything but `ping`.
* **Web routes** (cloud and agent-api profiles, signed-in only, the person is always the token's): `GET /api/v1/notifications`
  (`unread`, `snoozed`, `limit` up to 200, `before` cursor), `GET .../count`, `POST .../read-all`, `POST .../{id}/read|unread|snooze|ack`,
  `DELETE .../{id}`, and `GET /api/v1/approvals` now merges tool-call approvals (owner only) and tenant approvals (eligible approvers only)
  with a `kind` field and a `decide` hint; decisions use the existing endpoints.
* **Web UI**: `notifications.js` (page: approvals waiting, filters, bold unread, read/unread, acknowledge, snooze, delete, load older, the
  four states) and `bell.js` (top-bar bell, red while an approval waits, event stream with a 30 s polling fallback and a growing retry
  pause). No inline script, no library, phone width.

## 3. Channels

| Channel | Config | Notes |
|---|---|---|
| `inbox` | always on | the durable store; nothing to configure |
| `webhook` | `notify.channel` type `webhook` | https only, no credentials in the address, signed with HMAC-SHA256 over `<ts>.<body>` (`X-Iyakki-Timestamp`, `X-Iyakki-Signature: sha256=...`, the flows gate verifies it), 10 s timeout, payload at most 16 KiB, the answer's body is never read into the queue or the log |
| `email` | `notify.channel` type `email` | own SMTP client (`smtp.rs`): implicit TLS (465) or STARTTLS (587) only, AUTH PLAIN or LOGIN with a vault secret, one message per recipient, `From` fixed by the channel, text/plain UTF-8 as base64, header injection refused (not cleaned) before a byte is sent |
| browser Web Push | not built | needs a valid HTTPS certificate on a public name (a self-signed certificate cannot register a service worker or push subscription), a service worker file, VAPID keys in the vault and a push-service egress rule. Plan it with the connector app decision; do not build it before the cloud has real certificates |

Configuration is two tenant-config kinds, both off unless an item is switched on:

* `notify.channel` (collection, keyed per team and name): `type`, `min_severity`, `include_details` (default false), `details_ack`,
  `max_per_hour`, `link_base`, `url`, `signing_secret` (vault reference), `host`, `port`, `tls`, `from`, `username`, `password` (vault
  reference), `addresses` (`person=address`).
* `notify.routing` (single, tenant or team): quiet hours, time zone offset, digest time, escalation.

## 4. Threat model

Notifications may carry sensitive text. A channel sends data out of the tenant, and cannot take it back.

| Threat | Control |
|---|---|
| Data leaves the tenant by accident | Every channel is OFF until an admin switches it on; saving it shows the warning "data leaves your tenant" (`notify.data.leaves`), which must be acknowledged; the host must pass the tenant egress allow-list, and a tenant with no egress policy sends nothing (`RealNet::egress` needs `Some(true)`) |
| Message text leaks business data | By default a channel sends only a generic summary and a link to the Notifications page (`include_details: false`). Titles and bodies go out only with `include_details` AND `details_ack`; without the acknowledgement the channel stays link-only and saving is blocked (`notify.details.ack.missing`) |
| Secrets or vault values in a body | `redact.rs` runs before anything is stored or sent: the chat's credential detector (`credguard`), vault references, passwords inside URLs, control characters; a length cut |
| An action link that approves for whoever has the message | No such link exists. Messages carry the Notifications page address only; deciding needs a signed-in click |
| Forged or replayed webhook | HMAC-SHA256 with timestamp, the same scheme the flows gate checks (5 minute window, replay memory on the receiving side) |
| Plaintext mail or credentials | Only implicit TLS or STARTTLS exist as modes; the client refuses to send credentials or a message on a connection that is not TLS, never falls back, and asks for the capabilities again after STARTTLS |
| Header injection | CR, LF, NUL or control characters in an address or subject are refused before any byte goes out; From is fixed by config |
| SSRF through a channel address | https only, no userinfo, egress allow-list by host name, and the resolved address must be public (no loopback, private, link-local, CGNAT) |
| Flood (a flow that fires all night) | per-channel and per-recipient hourly caps, dedupe, bounded queue, dead letters |
| Reading someone else's notifications | the person is the token's; routes take no user name; ids of other people are 404; the volume is the tenant the session works in; the stream carries only the owner's events |
| Two replicas deliver twice | the lease; only the leader runs the worker |
| Tenant mix-up | the hub refuses a notification whose `tenant` is not its own; each tenant has its own volume, hub, bus and lease |
| Locked vault | deliveries that need a secret wait and retry; nothing is sent unsigned |

Open: the egress allow-list is checked by host name before connecting; the connection then resolves the name again (a DNS rebinding
window between the two). The public-address check on the resolved address closes the private-range case but not a public-to-public switch.

## 5. What a real test still needs

* A real mail server (STARTTLS and implicit TLS, AUTH PLAIN and LOGIN) and a real https webhook receiver. Everything here ran against
  fakes and a scripted SMTP server. `transport.rs` (`RealNet`) has no test against a network.
* A real browser for the WebSocket over TLS, the CSP (`connect-src 'self'` and `wss:` on Safari), the bell on a phone, and screen
  readers.
* The tenant's egress policy and vault bound in a pod.

## 6. Not built

* Web Push (section 3). A Settings page for channels (they are tenant-config items). A route and page for the per-person opt-out.
* Wiring the worker into the service process and the cloud flows' `NotifySink` onto `Hub` (section 8).
* Approval events pushed by the approval store: the stream's `approval` event today reports the tool-call count; a tenant request
  arrives as a normal `notification` (the worker announces it) and the page re-reads the list.

## 7. Safety rules added to tenant config

`notify.details.ack.missing` (block), `notify.plaintext` (block), `egress.domain.blocked` (block, existing rule extended to the channel
host), `secret.inline`/`secret.foreign` (existing), `notify.data.leaves`, `notify.details.on`, `notify.addresses.missing`,
`notify.rate.high`, `notify.egress.none` (warnings that must be acknowledged).

## 8. Wiring still to do by the owners of those files

1. Start a ticker in the tenant service process: `notifyhub::service::hub_for(&data_dir).tick(now, &RealNet {dir, ca_files: vec![]})`
   every 10 s, with no session needed. `hub_for` takes the tenant id from the volume's directory name; the supervisor should build
   the hub with `Hub::new(dir, <tenant id>, Box::new(DirEnv {dir}), <pod name>)` so the id and the lease holder are the real ones.
2. Implement the cloud flows' `NotifySink` by calling `Hub::emit` (the contract is `sink::Notification`).
3. Make the tenant-config discovery list show the two new kinds (they are in the registry and API already).
