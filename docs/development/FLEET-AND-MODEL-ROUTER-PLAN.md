# Fleet and model router: plan for alpha.4

Status: plan, written 2026-11-14 from the owner's request. Nothing in this document is built. It says what exists today, what the two features are, how they follow
the core rule ([LANE-RULES.md](LANE-RULES.md), "The core rule": everything goes through the IYAKKI core, which asks for permission and logs), the slices in the order
to build them, and what stays open. Matrix rows: `F-FLEET-01` to `F-FLEET-09` and `F-ROUTER-01` to `F-ROUTER-08`, all `planned`.

## 1. What the owner asked for

**Fleet.** Two or more IYAKKI systems pair and talk.

* A pair group with an id, like a cluster id.
* Start with IDE work only.
* Members take tasks and work on them; they share context and know what the others work on.
* On completion a worker shares its branch with whoever assigned the work.
* A worker node periodically shares its disk space, CPU and memory use.
* The master plans the work (it can be a worker too, if it has enough resources), integrates the changes, runs the tests and finalizes.

**Model router.**

* A bigger model plans: creates features and dependencies, provides the steps, assigns them to a lighter model.
* A lighter model does the light work of executing and fixing; depending on the model type it may take a photo or a sound. When blocked it asks the bigger model and gets a suggestion or solution.
* All of it is a **plugin**, so a user can provide their own strategy.

## 2. What exists today (and is reused)

| Piece | Where | What it gives the plan |
|---|---|---|
| Parallel tasks in the AI IDE | `crates/iyakki-agent/src/tasks.rs`, `tasks/plan.rs`, `tasktools.rs`, `iyakki-shell/src/apps/ide_tasks.rs`, [PARALLEL-TASKS.md](../PARALLEL-TASKS.md); rows F-IDE-10, 14, 15, 17, 21, 22, 23 | a task = own worktree and branch, own chat session, budgets and status; a lead plan fans out into tasks with dependencies; a memory-aware cap; a lead summary; review and merge |
| Shared project context | F-IDE-13, F-IDE-16 | a Context document every task sees |
| Board and dependencies | F-IDE-14 | rows with `Depends`; agents on the board with stable ids (F-IDE-15) |
| A task cluster abstraction | `TaskCluster` (kubectl-backed) in the IDE tasks code | tasks can already run on a cluster; the fleet is a second kind of place to run a task |
| Swarm | `crates/iyakki-swarm` (libp2p, mDNS, Noise, Yamux) and its scheduler, rows F-NET-03, F-NET-04 | LAN discovery, encrypted channel, least-loaded or fastest peer choice (today for WASM tasks that can only compute) |
| Developer-mode pairing | `iyakki dev pair`, Settings, Developer; keys per paired computer | a key-based way to pair two computers and revoke one |
| Sysinfo through the core API | `Os::sysinfo()`, `Memory`, `Disks` (F-ARCH-05/06) | disk, CPU and memory figures without reading `/proc` in the app |
| Model providers and profiles | F-AI-01, F-AI-09/10, F-AIHW-01/02, F-FLOW-32 (named assistant profile with models, local first, fallback, per-model limits) | the list of models, where each runs, what each type can do, token budgets and `BudgetStop` |
| Capabilities and approvals | [CORE-ACCESS-MODEL.md](CORE-ACCESS-MODEL.md), `GuardedOs` | a way to ask for `camera.view`, `microphone.listen`, `net.connect` with a card |
| Plugin mechanism | tiles, bundles, extensions ([APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md)); WASM tile sandbox | a sandboxed place to run a user's strategy |

## 3. Fleet

### 3.1 The pair group

* A **fleet** has an id (128 random bits, shown as a short word group for people), a name, a member list and a **group key**. A system can be in several fleets.
* **Create** on one system; **join** by pairing: the person on the new system enters a one-time code shown on a member (the same flow shape as developer-mode pairing and the browser client's one-time link),
  both sides check each other's key, and an administrator on each side approves. A member can **leave**; any administrator member can **remove** another; removing rotates the group key.
* The group record is signed by the creating member; every member holds the list and the keys of the others, so two members can talk when the creator is away.
* Roles are not fixed: any member can be a **planner** for one piece of work and a **worker** for another. A member says what it offers (models, GPU, free disk and memory, how many tasks at once).

### 3.2 Transport

* On the LAN: the swarm's discovery and Noise channel (mDNS finds members, the group key and member keys authenticate them). Beyond the LAN: an explicit address over the TLS channel of developer mode (slice 5).
* Messages are small JSON records (assignment, status, heartbeat, question, result pointer); large things (a branch, a context file) travel as a Git bundle or a file over the same channel with a hash.
* Nothing here is a new way for an app to touch the network: the channel belongs to the core, and an app uses it through the core API (section 5).

### 3.3 Work

1. The planner makes a plan (the existing lead plan fan-out, F-IDE-21): rows, dependencies, a goal and files per step.
2. For each ready step it **assigns** a task to a member: title, goal, board rows, a digest of the shared context, the base commit and branch, a budget, the class of model wanted. The choice uses the members' **heartbeats** (free memory and disk, load, models, how many tasks run) and the same memory-aware cap rule of F-IDE-23, applied per member.
3. The worker runs it as an ordinary local parallel task (own worktree, own chat session, own approvals: a question that needs a person goes to **that member's** person, and also shows on the planner's queue as "waiting on X").
4. **Heartbeats** every 10 to 30 seconds (a change of more than a few percent, or on request): disk free, CPU load, memory used, tasks running, models available. They carry numbers only, no paths or content.
5. On completion the worker **hands back** its branch: a Git bundle of the task branch since the base commit, with the head hash and the test result it saw. A failure or a question goes back the same way as a message.
6. The planner **integrates**: it applies the bundles to an integration branch in dependency order, runs the tests, and finalizes (or sends a conflict or a failing test back as a new task). The planner may be a worker too when its own resources allow (the same cap).

### 3.4 Shared context and awareness

* The project Context document (F-IDE-13) is shared: a change by any member is a record every member receives; conflicts keep both versions for a person to resolve (no silent overwrite).
* Every member's Board shows which rows are with which member and their status, from the status messages; a member that goes silent for N heartbeats is shown as lost and its running tasks are requeued by the planner (idempotent: a task has a stable id, a returned bundle for a task already finished is dropped).

### 3.5 Safety

* A worker keeps all its own protections: the task runs in its isolated worktree with its own approvals and budgets. A planner cannot approve anything on a worker's behalf, cannot raise a worker's capabilities, and cannot read a worker's files outside the task branch.
* No secrets travel: vault entries are never part of an assignment; a task that needs a credential uses the worker's own vault by name.
* Every fleet action is an audited, capability-checked call (section 5). A removed member's key stops working at the next message; the group key rotates.

### 3.6 Slices (alpha.4)

1. **Pair and see**: create, join, list, leave; heartbeats shown in the Board's member strip. Two systems on one LAN; tests with a fake transport and two real loopback nodes.
2. **Assign and hand back**: one task from the planner to one worker; the worker's branch comes back as a bundle; the planner shows it for review.
3. **Shared context and awareness**: context sync, rows with members, lost-member requeue.
4. **Integrate**: the planner applies bundles in order, runs the tests, finalizes; conflicts become tasks.
5. **Beyond the LAN** and relay.
6. Chat tools and Settings page for all of it (section 5).

## 4. Model router

### 4.1 Model registry

One list of the models the person can use (providers, local and remote, and, with the fleet, those on other members), each with: text, vision, audio in, audio out, tool calling, context size, speed, cost, **where it runs** (this computer, the network, the internet: the privacy zone), and whether it is local. It comes from the existing provider and hardware-profile code; the router adds no new way to reach a model.

### 4.2 Roles

* **Planner model (bigger).** Given a goal, it produces features, **dependencies** and **steps**, each with an acceptance check and the kind of model it needs, and assigns steps to executors. It does not run the steps.
* **Executor model (lighter).** Executes and fixes. According to its type it may use a camera or a microphone (taking a photo, listening) **through the core API with the usual approvals** (`camera.view`, `microphone.listen`): the router never grants a device.
* **Escalation.** When an executor is blocked (a failing test after N tries, a question it cannot answer, a tool refusal that needs judgement) it sends the planner a short record (what it tried, the error, what it needs). The planner answers with a suggestion or a revised step. Bounded: a maximum number of rounds and a token budget per step; past it the step goes to a person.

### 4.3 The strategy is a plugin

* A **strategy** is a plugin with one small interface: input JSON in, decision JSON out, no side effects.
  * Input: the task or the blocked step, the registry (models with their capabilities, costs and zones), the budget left, the history of this step, the fleet members and their load.
  * Output: a decision: which model (or which member) takes the step, with which tools and budget; or "escalate to model M with this question"; or "ask a person".
* It runs in the **sandbox of the tile host** (the WASM sandbox, or a web-bundle script) with **no capabilities**: it cannot call a model or a tool itself, it can only decide. The core carries out the decision and logs it.
* The **built-in default strategy**: the bigger model plans, the lightest model that has the needed abilities and fits the budget executes, escalate to the planner on a block, ask a person after the round limit.
* A person selects a strategy per workspace (and per fleet), can write one with [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md) (kind: extension), and can see every decision it made.

### 4.4 Safety and cost

* Privacy zones are respected by the core, whatever the strategy says: a model whose zone the person did not allow is never chosen (a strategy that picks one gets a refusal that is logged).
* Budgets: the existing per-app and per-model limits and `BudgetStop` apply on top of the router's own step budgets; a strategy cannot raise a limit.
* Every routing decision (input digest, output, who carried it out) is in the access log; no message content.
* Approvals are unchanged: a model asking for a tool that asks first still asks the person.

### 4.5 Slices (alpha.4)

1. **Registry and decision API** in the core (read-only): the models and their abilities; the decision record.
2. **Default strategy** with planner and executor roles inside one machine; escalation with round and budget limits; tests on fake models.
3. **Strategy plugin interface**: the sandbox, the JSON interface, selecting a strategy, the decision log; a sample strategy plugin.
4. **Multimodal executors**: photo and sound through the existing approvals.
5. **Router plus fleet**: choose a member by its models and load (joins both parts).
6. Chat tools and Settings page.

## 5. The core rule applied

* The fleet channel, the heartbeat reader and the router's model calls are **core API methods** (`Fleet`, `Router`, with `FakeOs` and fake implementations), not code an app reaches around. The IDE tile and the assistant tools call them through `cx`.
* New capabilities (names to be fixed in the catalogue when built): `fleet.read` (members and their numbers), `fleet.join` and `fleet.manage` (hard ask: a card on every pairing or removal), `fleet.assign` (send work to a member; a card per assignment batch), `fleet.receive` (accept work from a member; the person on the worker sets the default and can say "ask me for each task"), `router.use`, `router.strategy.set`.
* **Everything reachable by chat and by the screen**: chat tools `fleet_status`, `fleet_pair`, `fleet_leave`, `fleet_assign`, `router_status`, `router_strategy_set`; a Settings page for the fleet (members, offers, heartbeats, the accept-work switch) and one for the router (registry, strategy, decisions). Changes ask first.
* A new app or tile built for this follows [APP-DEVELOPMENT-PROCEDURE.md](APP-DEVELOPMENT-PROCEDURE.md), including its isolation tests.

## 6. Open questions for the owner

1. **Trust inside a fleet**: should a worker accept work from any member by default, or ask its person for each assignment (my recommendation: ask by default, with an "accept from this member for this project" switch)?
2. **Where the integration branch lives**: on the planner only (my recommendation) or also pushed to a shared remote?
3. **Cost**: a fleet that runs paid remote models on several members can multiply spend; should the router's budgets be per fleet as well as per member (recommended)?
4. **Strategy language**: WASM (any language, strict) or a web-bundle script (simplest to write)? My recommendation is the web-bundle script first, WASM when the interface has settled.
5. **Scope of "start with IDE work"**: confirm that flows and the assistant's own long runs stay out of the first alpha.4 slices.

## 7. What this plan does not claim

No code exists for the fleet or the router. The reused pieces (parallel tasks, swarm, pairing, providers, sandbox) are partial or tested only on fakes, as their rows say; a first slice must say plainly what it ran on real machines.
