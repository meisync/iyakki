# Security policy

IYAKKI is an operating system for AI agents: it runs code, holds credentials and talks to networks and clusters on a person's behalf.
Security reports are welcome and taken seriously. This is an **alpha**, so read "What the alpha does not promise" first.

## Supported versions

| Version | Supported |
|---|---|
| 0.1.0-alpha.N (latest alpha) | Fixes go into the next alpha. Only the newest alpha is supported. |
| Anything older, any fork | Not supported. |

There are no long-term-support branches and no backports before 1.0.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.

* Mailbox: **OWNER: fill in** (a monitored address, for example `security@your-domain`; this placeholder must be replaced before the
  release is announced).
* Encrypted mail: **OWNER: fill in** the public key or fingerprint if you want to offer one, or delete this line.
* Include: the version (`iyakki --version`, or SYSTEM, About), the image variant, what you did, what you expected, what happened, and
  whether the issue needs physical access, a signed-in person, or the network.
* You will get an acknowledgement within **OWNER: fill in** (suggested: 7 days) and a decision on severity and a fix plan afterwards.
  Please allow a reasonable time (suggested: 90 days) before publishing details; credit is given if you want it.

For everything that is not a vulnerability use the normal channels in [SUPPORT.md](SUPPORT.md).

## Threat model in short

The design is in [BOUNDARY.md](BOUNDARY.md) (what the supervisor and the shell guard, and where the line is) and in
[Agent OS Architecture Specification.md](Agent%20OS%20Architecture%20Specification.md). In summary:

* **Agents are untrusted.** Model output, tool results and web pages are treated as untrusted data. Tool calls go through roles,
  scopes, per-run budgets and approvals (approve / always / deny); file writes show the real diff; guests run in a WebAssembly sandbox
  with explicit grants and a fuel budget; network access goes through an allow-listed egress proxy.
* **Secrets live in vaults** (XChaCha20-Poly1305, Argon2id). The assistant is told that a secret exists, never its value; the chat
  input guard warns about pasted keys (a heuristic).
* **Accounts and roles** (administrator, operator, user, guest) are enforced in code, not only in the interface; settings changes are
  re-checked and audited in a hash-chained log.
* **Packages and catalogs** are signed (ed25519) and shown with a trust level before install; revocation lists only move forward.
* **Releases** are signed: `SHA256SUMS`, `release.json` and their signatures; verify before you write an image
  (see [docs/RELEASING.md](docs/RELEASING.md)).
* Licensing and supply chain: [docs/LICENSES.md](docs/LICENSES.md), `THIRD_PARTY_LICENSES.txt`, `SBOM.json`.

Out of scope of the model today: a person with physical access to the machine or the unencrypted data volume, a malicious
administrator, side channels, and kernel or firmware bugs in the base system.

## What the alpha does not promise

* **No security review** by anyone outside the project, no penetration test, no fuzzing campaign.
* **No Secure Boot**, no measured boot, no disk encryption of the data volume (vaults are encrypted, the volume is not).
* **No stability of formats**: vaults, sessions, ledgers, package and catalog formats can change between alphas without migration.
* **The official store root key does not exist yet**: nothing but items built into the image can be "Official", and the update and store
  channels have no production trust anchors.
* The browser front end uses a self-generated HTTPS certificate; put it behind your own TLS and access control if you expose it at all.
* The release signing key is new. Its public half is published in the README and on the download page; verify it out of band.
* Heuristic protections (credential guard, refusal and advice detectors, English phrase tables) can be bypassed.
* The coding-agent bridge, the dashboards' connectors and the cloud model providers were not checked against the real services (see
  [KNOWN-ISSUES.md](KNOWN-ISSUES.md)).

If you run it, do so on a machine and a network where a mistake is cheap.
