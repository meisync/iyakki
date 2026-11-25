# Security of the UI socket (`ui.sock`)

**For:** reviewers and developers who need to know who can talk to the core over the UI socket, and what stops everyone else.
**You will get:** what the socket is, the five layers that guard it, what never crosses it, and what it does not protect against.

Sources: `crates/iyakki-ui-client/src/server.rs` (the server loop), `crates/iyakki-agent/src/contract_local/uiserve.rs` (setup and configuration),
`crates/iyakki-core-api/src/appcaps.rs` (the signed capability table), `docs/development/UI-BOUNDARY.md` and `docs/development/ALPHA-7-DECISIONS.md`
(D-SPLIT-03-HANDLE, D-SPLIT-03-ENV, D-SPLIT-07-PUSH).

## What it is

The core (process 1) owns everything privileged: disks, accounts, secrets, hardware, the audit log. The shell and the apps run in a separate program,
`iyakki-ui`. The two talk over a **Unix domain socket**: a file on the machine, `/run/iyakki/ui/ui.sock`. Nothing goes over the network and no TCP port is
opened. The same contract the web service and the developer console speak travels over it as lines of JSON: a `hello`, calls, pushed events, bulk blocks, and
(when `rpc` is on) calls on the core's service objects.

## The five layers

1. **File system.** The core creates the folder with mode `0700` and the socket with mode `0600`, and removes a stale socket left by a crashed run. Only the
   owner and root can open the socket.
2. **Kernel-named caller.** On every new connection the core reads the peer's user id from the kernel (`SO_PEERCRED`). A peer that is neither root nor the
   socket's owner is dropped before one byte is read. A program cannot claim another identity: the kernel supplies it.
3. **Off by default, fixed scopes.** The socket serves only when its configuration says `enabled: true`. Every caller on it holds the one list of contract
   scopes in the configuration (for example `flows:read`, `events:read`). A call outside the list is refused, and a listener without `flows:read` cannot
   subscribe to flows. The socket acts as one named account (`ui`), as a plain user unless an administrator sets `admin`.
4. **The core's identity, not the caller's.**
   * The UI program is the app `client.ui`. It has its own row in the signed built-in capability table: 12 `events.*` scopes, no `events.files`, no
     `flows.start`, no hard asks.
   * **A claimed right is never read.** Every served call whose arguments claim a right (a policy, a role, an accounts session, an "is administrator"
     flag, the list of administrators, the personal-skill flags) is listed in one table, `crates/iyakki-ui-rpc/src/served_auth.rs` (`RULES`). Before the
     call runs, the core takes the NAME of the account, reads that account from its own accounts and writes its own reading over the claim; a name it does
     not know is refused and the call never runs (D-SEC1-VOUCH). The accounts handle does the same one level down (`AccountsGuarded` re-reads the actor
     of a change by name), so a forged "guest is admin" session is refused twice. The calls that never carried a right (the radio search, the study PDF,
     the download queue, the store) send only the name (D-SPLIT-03-POLICY, D-SPLIT-03-STORE).
   * Changes to the machine are made by the core itself, through the broker. The broker's table and access log still decide each change.
5. **Hard limits and what never crosses.**
   * At most 32 connections at once; a 60-second read timeout.
   * **Pace.** A socket that serves the service objects (`rpc`) gives the first-party UI program `FIRST_PARTY_RATE`: 200,000 calls a second, in place of the
     catalogue's 120 calls a minute meant for third-party programs (D-SPLIT-07-RATE). The shell reads hundreds of values a second, so the low figure would
     starve it. The pace is therefore not a defence on this socket; layers 1 to 4 are. The owner has not yet reviewed this choice.
   * **Who switches it on.** The core hands the socket to the program it supervises and restarts that program if it exits (`uiserve::hand_over`,
     D-SPLIT-03-SOCKET). The program cannot enable the socket itself.
   * The service objects (`rpc.call`, the served `ServicesHandle`) are served only when `rpc` is set, for the first-party UI program, and are off by default.
   * Password hashes cross as `*` (a password exists) or empty (none). The vault's secret values, `agent_vault_token`, and the git keys' secret store stay off
     the wire. `code_vault` seeds cross as `*`; a save carrying `*` keeps the core's secret and a `*` for an unknown entry is refused.
   * Calls that carry a UI callback or UI object are refused unless the core supplies its own (D-SPLIT-03-ENV); without the core's handle they answer
     "not served over the UI socket", never a made-up success.

## What this does not protect against

* **One trust level.** Any process running as the socket's owner (or root) holds the configured scopes. If an attacker already runs code as that user, this
  socket does not stop them. The UI program is part of the device's trust base (D-SPLIT-07-PUSH).
* **`resume_session` needs no password**, for the same reason. The password methods (`on_login`, `on_resume`, `unlock_user_vault`, `on_password_change`) are
  served over this socket as part of the trust base; a failed call answers a locked vault.
* **Not a network boundary.** Reaching the machine from another computer is the job of the web service, the developer console and the relay, which use TLS and
  their own scopes and approvals.

## What is tested, and what is not

Tested over real Unix sockets on the build host: the owner and mode of the socket, the refusal of a peer that is not allowed, scope refusal (no `flows:read`,
no `events:read`), forged and unknown actors, the hash staying off the wire, refused callbacks, and the connection limit.

Tested over a real loopback socket (`crates/iyakki-ui-rpc/tests/served_auth.rs`, alpha.7 SEC1), each with a forged "the guest is an administrator" claim:

* the five accounts calls that take a session (`create_user`, `delete_user`, `set_role`, `set_overrides`, `set_password`): refused, nothing written, with the
  guard and without it; an unknown actor refused; an administrator who calls itself a guest still is one;
* the policy of `scope_check_read`, `scope_check_write`, `scope_roots`, `scope_roots_with_home`, `rag_refresh_tick`, the flows calls, `StoreHub`,
  `StoreUpdates` and `DashHost::read_ok` becomes the named account's own policy; an unknown name is refused;
* the role of `ClusterFacet::switch` and `FleetApi::start`/`stop`, the admin flag of `on_login`, `on_resume`, `on_logout`, `sharing_notice_tick`, and the
  administrator list of `computer_announce` become the core's reading;
* the personal-skill flags of the skill and package calls become the account's `skills` right;
* the generated dispatchers decode what the core wrote (typed fakes), and the story store the UI builds holds the core's policy, not the caller's.
  (The `iyakki-compose` run against the real services, `remote_services`, repeats the file-rule case with a real accounts store.)

**The audit.** `served_auth_audit` (a unit test of the crate) reads every served method (the generated dispatchers, the hand-written ones, and the signatures
of the remote objects) and fails when a method decodes a `Policy`, `Role`, `PolicyOverride`, `Session`, `Actor` or `Who` from the wire, or has an argument
named like a right (`admin`, `admins`, `is_admin`, `can_*`, `approved`, `allowed`), that is neither a rule in `RULES` nor a reviewed entry in `ALLOWED`. Each
`ALLOWED` entry says why: `Data` (the thing asked for, not a right) or `Open` (a known gap, listed below). The audit is itself tested on synthetic text.

**Alpha.8 (SEC3, `crates/iyakki-ui-rpc/src/served_login.rs`, tests `crates/iyakki-ui-rpc/tests/served_login.rs`):**

* **The acting person is bound to the connection.** The core records, per connection, the account that signed in over it: a password the core checked itself (`AccountsApi::authenticate`, or `ServicesHandle::on_login` while nobody is bound), or `resume_session` / `on_resume` (no password, the limit below). A served call that names an account in a rule of `RULES` must name that account; another name is refused and audited, except `on_logout` by an administrator (`ADMIN_ACTS`). The five accounts calls must carry the session of the signed-in account. A connection signed in as one person cannot sign in or resume as another until `on_logout`. A connection nobody signed in on may not act as anyone. The binding ends at `on_logout` of that account or when the connection closes. It is on for the UI socket (`with_login_binding` in `iyakki-compose::rpc`).
* **The shared-scope skill right is read from the signed-in account** (`manage_skills`; `skills` for the personal flag), so `can_shared` is no longer a claim. The audit allow-list has no `Open` entries any more (a test keeps it so).
* **`approved` is a one-time ticket.** After the person approves on the card, the UI asks the core for a ticket (`Approval::issue`) and sends it as one trailing argument. The core removes it, then checks that it exists, is unspent, is not past 60 seconds on the host's clock, was issued to this account on this connection, for this call, with this canonical hash of the arguments. It is spent either way. An approved call without a valid ticket is refused (never run as "not approved"). Issue, use and refusal are audited (`ui.approval.*`, `ui.login.*`).

**Still true:** a compromised UI program can ask for a ticket for any plan it likes (the ticket makes approval a bounded, audited act of a known account, not a hardware proof), `resume_session` needs no password, and the story store judges paths locally over the roots the core gave. Closing these needs the approval to be shown by the core itself (a core-drawn confirm), which is not built.

**Not yet done:** a run on a booted device with the core supervising the `iyakki-ui` program (Manual: needed).

## If you change this

A new item on the wire, a new scope for `client.ui`, or a change to what `rpc` serves is a security change: list it for review (core-api items go through
`compat_surfaces` and `docs/development/core-surface.txt`), add a test over a real socket, and record the decision in
`docs/development/ALPHA-7-DECISIONS.md`.
