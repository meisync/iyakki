# Schedules and workflows in cloud mode

Owner question, 2026-11-04: "will schedule and workflow work on cloud mode? we should get it done as well." Later the same day:
notifications and approvals can be triggered by workflows; the user session may go away and scheduled workflows keep running;
sandboxes are wanted too (another plan, `crates/iyakki-agent/src/sandbox/`).

Marks used below: **[proven]** read in the code or shown by a test, **[guessed]** inferred or a decision taken here, **[not tried]**
needs a real cluster or browser. Nothing in this plan was run on a cluster.

## 1. The audit: what the cloud pod did before this work

| # | Question | Finding |
|---|---|---|
| 1 | Who starts the scheduler? | The engine ticks (and with it the scheduler) inside a `FlowEnv`, one per person (`flows/mod.rs`, `FlowEnv::open`, `auto_tick`). Only two things build one: the desktop shell at sign-in (`iyakki-shell/src/shell.rs`, `start_for_login`, in a thread) and a call that happens to need one (`flow_run`, a flow tool). **[proven]** In a headless pod `iyakki serve --profile cloud` runs no shell, so nothing started the scheduler until somebody called a flow route, and it stopped being started again after a restart until the next call. Answer: no, a schedule did not run at pod start with nobody signed in. Needed: per tenant. |
| 2 | Where does state live? | Flow definitions in `<home>/Flows/<id>/` (`scope::home_root`), run history and `sched.json` in `<data>/flows/<user>/` with `data` = `svc.dir()` = the tenant volume `/var/lib/iyakki` in the shipped manifest. **[proven]** So it is on the volume and survives a restart or reschedule, if the volume follows the pod (ReadWriteOnce PVC, `Recreate`, one replica). **[proven]** `sched.json` is written at every tick that changes it (atomic rename). **[proven]** Whether `home_root` for an account lands on the volume in a pod depends on the `/home/<user>` bind (`homebind`); not checked in a pod. **[not tried]** |
| 3 | Time zone | Cron triggers carry a `tz` (default `local`). `local` was `Zone::system()` and a named zone came from `/usr/share/zoneinfo`; the shipped `FROM scratch` image has no zone files. **[proven]** So a pod ran everything in UTC by accident and a name like `Europe/Berlin` silently meant UTC. Daylight saving is handled by the scheduler for any zone that gives offsets (`local_to_unix`: the earlier instant for an overlap, the next valid one for a gap). **[proven]** There is no time zone database in the repository and none was written. |
| 4 | Double run with replicas / rolling update | Nothing prevented it: two pods on one volume would each load `sched.json` and each fire. The shipped manifest runs one replica with `Recreate`, which avoids it except while the old pod finishes its 10 s grace period, and a user who scales to 2 gets doubles. **[proven]** |
| 5 | Missed runs after downtime | Handled by the scheduler per trigger: `skip`, `run once`, `catch up (max N)`, on the first tick after start, from the saved `next_ms`. **[proven]** (sched tests) It needs the scheduler to be started, which was point 1. |
| 6 | Execution | In process only: the engine's `exec` runs the command in the pod (`Local`) or as a k3s Job through the *desktop's* dev-cluster profile (`devcluster::resolve`), not the tenant's team binding. Limits: `max_runs` 8 per person, per-flow `max_concurrent_runs`, node timeouts. No tenant cap, no total time limit per run. **[proven]** |
| 7 | A flow that asks a person | The `approval` node blocks the run and sends an inbox notice to that person (`Host::notify`, `needs_you`). Nobody else could answer, nothing expired it unless the node had `timeout_s`, and it never reached the tenant's approvals inbox. **[proven]** |
| 8 | Secrets | `{{secret.NAME}}` is resolved by the host from the person's own vault (`uvault`) or the system vault; the engine registers the value with a redactor so logs and run files do not hold it. **[proven]** A person's vault is locked when they are away, so a scheduled run failed with "no secret". Team vault entries were not reachable. **[proven]** |
| 9 | Audit, retention | The host writes `flow.run.start`, `flow.approval`, `flow.schedule.failed` lines to the account's audit; schedule edits had no cloud audit line; `RunStore::prune` exists, and a limit by days did not. **[proven]** |
| 10 | Isolation | Per tenant by volume (one pod, one volume). Within a tenant, per person by home folder and `flows/<user>`. A flow cannot name another person's flow. **[proven]** for routes (`FlowEnv::open(user)`); the in-process `exec` of a flow runs as the pod user and could read the whole tenant volume. **[guessed]** (the Local runner's jail was not audited here; the sandbox plan covers it). |
| 11 | Observability | None. |
| 12 | Web API for schedules | Only `flows (read)`; since today also run start / run list / run view (another agent). Nothing for schedules. **[proven]** |

## 2. What was built (all in `crates/iyakki-agent/src/cloudflows/` unless said)

| Piece | File | Notes |
|---|---|---|
| Tenant scheduler service | `service.rs`, `real.rs` | `TenantScheduler::step` every 5 s from a thread started by the cloud profile (`Extras::background_start`, called by `webservice::run`): lease, start the engine of each account that has flows (no sign-in involved), owner checks, time limit watchdog, notifications, counters. Clean stop: engines halted (runs stay on disk as interrupted and are picked up by the next leader), lease released. |
| Leader lease | `lease.rs` | JSON file `<data>/cloud/flows/lease.json` (holder, epoch, heartbeat, ttl 30 s), atomic rename and read-back; a heartbeat that finds another name stops everything. Limits written in the file's header: network file systems with client caching can show a stale read for some seconds, so keep ReadWriteOnce. |
| Engine hooks | `iyakki-flow/src/engine.rs`, `sched.rs`, `flows/mod.rs` | `EngineCfg::gate` (`lead`, `admit`) and `local_tz`; `Scheduler::reload`. A replica without the lease still serves manual runs but fires nothing on a timer; a replica that becomes leader reloads `sched.json` so a tick the old leader fired is not fired again; a queued run is held back (not failed) while `admit` says no. `flows::CLOUD_HOOK` applies them to every account's environment, including the ones the routes open. |
| Time zone | `tz.rs` | `schedule.timezone` (default UTC): UTC, fixed offsets always; a zone name only when the image has zone files, else UTC and `setting_ok` says so. `local` in a cron trigger means the tenant's setting. No rule table was invented. |
| Quotas | `quota.rs`, kind `flows.quota` | Runs at once per tenant and per person, run time limit, schedules per tenant, history size and days, "missed" threshold. Defaults 4, 2, 3600 s, 50, 200, 30 d, 900 s. A team entry can only lower a value. |
| Execution | `executor.rs`, `job.rs` | `Executor` trait; `InProcess` (quota, deadline) and `K3sJob` (quota, `k3s_decide` for the team's binding and environment, promotion order, four eyes, the approval router; the Job manifest as text). A scheduled run in an environment that needs approval returns `Wait`, files one router request (kind `k3s.cluster`, requester = the schedule owner) and uses the approval once. The Job comes from the sandbox layer's single builder (`sandbox::job::bundle`, after `spec_from_flow_job` and admission by `sandbox.policy`), so it has the NetworkPolicy, workspace and runtime class too; a Kubernetes Secret reference is refused (vault references only). Nothing applies the documents: **the K3sJob executor is not yet called by the engine**. In a cloud pod a flow's `cli` and `browser` steps and `run_command` go through `sandbox::wiring` (`flows::CLOUD_EXEC` / `CLOUD_BROWSER`, installed by the scheduler thread; refused when not installed). |
| Approvals from a flow | `approval.rs`, `tenantconfig/approvals.rs` | A blocked `approval` node becomes a request of kind `flow.approval` (item `<run>_<node>`), routed by the existing router (approver-role holders, then the team's admins without the requester, then tenant admins, never the requester); decided through `POST /api/v1/tenant/approvals/{id}/approve\|deny` from any session; the poll carries it to the node exactly once (claim file, `O_EXCL`); `on_timeout` approve / reject / fail (default reject) when the node's timeout or the request's expiry comes first. The hub announces a waiting request to the approvers and sends the reminders (it reads the approvals file); this module sends only the "expired, handled as X" notice to the approvers and the requester. If nobody is eligible the node is rejected at once. |
| Notifications | `notify.rs`, `real.rs` | A thin adapter over the tenant notification hub (`notifyhub`, another agent's: durable outbox, dedupe, channels, quiet hours, reminders, escalation). `from_node` maps a flow `notify` node (`to` = me / owner / team / role:admin / role:tenant_admin / role:approver:<scope> / role:group:<name> / people, `severity`, `title`, `body`) to the hub's `Notification` with source `flow:<id>:<run>`; `render` fills run data and refuses secrets and vault references; `say` is what the scheduler uses. The service thread uses `hub_for(data)` as the sink and runs the hub's delivery worker (`Hub::tick` every 10 s). `Host::notify` in a cloud pod goes to the hub (`flows::CLOUD_NOTIFY`) except steps that wait for an approval. The engine's `Notice` has no `to` field, so a notice reaches only the person the run belongs to; honouring a node's own `to` needs `Notice` extended (not done). |
| Run identity | `identity.rs` | Schedule record: owner, team, environment, `run_as` (`owner` or `team-service`), the owner's role snapshot. Checked against snapshot and current membership; a lower or missing role pauses and tells the team admins. Run tokens: random, in memory, scoped to one run, expiring, never written. Secrets without a person: only `flows.unattended` entries (a team admin marks one secret, with an acknowledgement); locked, missing or unmarked gives `blocked: needs secret` plus a notification. Personal Claude / Copilot sign-ins are refused at save time. |
| Schedule API | `webservice/schedules.rs`, `routes.rs`, `deps.rs`, `real.rs`, `openapi.rs` | `GET /api/v1/schedules`, `POST /api/v1/schedules` (create or change), `DELETE /api/v1/schedules/{flow}/{trigger}`, `POST .../{flow}/pause`, `.../resume`, `POST /api/v1/schedules/preview`, `GET /api/v1/schedules/status`. Run now and history are the existing `/api/v1/flows/{id}/run` and `/runs`. |
| Web page | `webservice/ui/schedules.js` | List with state words, form with cron preview from the service (tenant zone), pause, resume, run now, change, delete, history link, the four states, phone width, no outside code. |
| Tenant configuration | `tenantconfig/schema.rs` | New kinds `flows.quota`, `schedule.timezone`, `flows.unattended`. |

Audit lines: `flow.schedule.save|delete|pause|resume` (tenant audit file and the service audit), `flow.schedule.paused` (service), the
approval router's `tenant.approval.request|approve|deny|close`, `tenant.k3s.deploy` for Job decisions, `flow.approval.resolved`. No
line holds a secret or a token.

## 3. Properties and how they are kept

1. **Schedules belong to the tenant service, not to a session.** The thread starts with the cloud profile; accounts are found on the
   volume; no function in `cloudflows/` takes a token or a session. Tested: `schedules_start_with_no_person_signed_in`,
   `a_run_token_is_scoped_to_one_run_expires_and_is_never_written_to_disk` (greps the state files for tokens and "session").
2. **One leader.** `two_replicas_never_both_lead_and_the_loser_stops_its_engines`, `lease_is_taken_over_only_after_it_expires`, and in
   the engine `a_replica_without_the_lease_starts_nothing_on_a_timer_and_a_new_leader_does_not_repeat_a_tick`.
3. **No stale rights.** `an_owner_who_left_the_team_pauses_their_schedules_and_the_admins_are_told`, `a_lowered_role_pauses_...`.
4. **Secrets.** `an_unattended_run_may_use_only_a_marked_team_secret_and_only_when_the_vault_is_open`. How the tenant vault is open
   with nobody present: the existing seam is `tenantconfig::vault::for_user(data, user)`, which reads a per-member key file
   `<data>/cloud/vault-keys/<user>.key` and the pod's `IYAKKI_VAULT_URL`. The scheduler asks for the service identity `_flows`; **its key
   has to be supplied by the operator** (a Kubernetes Secret copied or mounted to that path at pod start). That enrolment is **not built**:
   until it is, the vault reads as locked and unattended runs that need a secret go `blocked: needs secret`. No crypto was added.
5. **Approvals outlive sessions.** `a_decision_from_another_session_resolves_the_waiting_node_once`,
   `a_pod_restart_keeps_pending_approvals_and_finishes_a_half_applied_decision`, `a_timeout_applies_on_timeout_approve_reject_or_fail`. Reminders at half and nine tenths of the time are the hub's (its tests), not tested here.
6. **Double decision.** `two_paths_deciding_at_the_same_time_resolve_the_node_exactly_once` (8 threads). The INBOX path of the desktop
   keeps calling `Engine::decide` (single user, the node must be waiting); routing it through `Waits::resolve_once` is **not wired**.

## 4. Quotas, limits, retention

Concurrency: the engine of each person is capped by `max_runs_person`; the whole tenant by `admit` (counts the running runs of every
engine in the process; if the table is busy it falls back to the person cap). Time: the watchdog stops a run after `max_run_secs`
(cancel, then a notification). Schedules per tenant: `max_schedules`, refused with 429. History: `history_keep` / `history_days` are
in the configuration, **but pruning by them is not wired** (the engine's existing keep-count is unchanged). Tenant isolation: one
volume per tenant; tests show two data directories share nothing (leases, registry, waits, approvals, notifications).

## 5. Not built, or built but not wired (honest list)

* The `K3sJob` executor and the Job documents exist and are tested as text (golden file; `kubectl create --dry-run=client` reads it).
  A flow `cli` step runs through the sandbox's own executor (`CloudExec`), not through `K3sJob`; making a scheduled run use `K3sJob`
  needs the apply step and is the same work as the sandbox layer's Job executor. The team for a `cli` step is the person's highest-rank team (guess).
* History pruning by days and count; a pause by a team admin of someone else's schedule (the routes work on the caller's own flows).
* The desktop INBOX approve path through `resolve_once`; `quota` refusals are counted when a queued run is held back, not on a per-flow basis.
* Which team a flow belongs to: there is no flow-to-team field. A schedule records a team chosen at save time; a blocked approval
  from a manual run uses the first team the owner is in (guess). A flow with an `approval` node and no team is rejected (default deny).
* `ServicesExtras` opens a `FlowEnv` through the account's `Policy` from `Authenticator::lookup`; the thread needs the accounts to exist in the pod's account store.
* Real cluster, real pod restart, real network volume, a real browser, the inbox bell showing the notices, a second replica: **[not tried]**.

## 6. What needs a real cluster to verify

1. Two pods on one ReadWriteMany volume: lease behaviour with NFS caching.
2. A pod killed and rescheduled while a run is blocked on an approval; the run comes back, the approval is still pending.
3. The vault key for `_flows` supplied from a Kubernetes Secret.
4. A Job created from the golden manifest in `<team>-<environment>` namespace with the binding's credentials (namespace naming is a guess).
5. The page `schedules.js` in a browser on a phone width, with the CSP.
