# Shared vault cryptography: what it does and the options

Status: written 2026-11-06 for row F-CLOUD-15. Owner decision of the same day: IYAKKI does not wait for, or pay for, a review of
the shared vault's cryptography. Instead, whoever runs the system decides which cryptography is used, including their own existing
system.

**The built-in shared vault is not externally reviewed.** No cryptographer outside the project has read the design or the code. The
primitives are standard and come from widely used crates; the way they are put together (below) is ours. Treat it accordingly.

## 1. What the built-in shared vault does today

Code: `cloudkeys.rs` (keys, wrapping, sealing), `cloudvaultd.rs` (the service), `cloudrotate.rs` (rotation), `cloudvaultguard.rs`
(replay and rate limits), `cloudsnapshot.rs` (snapshot keys). The personal and system vaults of a machine (`vault.rs`) are a
different, older design (Argon2id-protected key slots) and are not what this document is about.

**Scopes.** `tenant`, `namespace:<ns>` and `common`. Each scope has one 32-byte random **scope key** and a **generation** number.

**Member keys.** A member has one 32-byte random secret, kept on the member's side (`cloud/vault-keys/NAME.key`, mode 0600). Two key
pairs are derived from it with HKDF-SHA256 (salt `iyakki-cloudkeys-member-v1`, info `wrap` or `sign`): an X25519 pair to receive keys and
an Ed25519 pair to sign requests. The service stores only the two public keys.

**Wrapping a scope key for a member.** A fresh X25519 key pair per wrap, Diffie-Hellman with the member's public key, HKDF-SHA256
(salt = ephemeral public key and recipient public key, info `iyakki-cloudkeys-wrap-v1`) to a 32-byte key and a 24-byte nonce, then
XChaCha20-Poly1305. The scope, the member's name and the generation are bound as associated data. An all-zero shared secret is refused.

**Sealing a value.** XChaCha20-Poly1305 with the scope key, a random 24-byte nonce, and associated data of the scope, the secret's
name and the generation. Sealing and opening happen on the member's side; the service only sees ciphertext.

**Who may do what.** Decided by the service on every request from the tenant's `roles.json` (owner, admin, member, namespace
admin/editor/viewer), and a wrap is needed as well as the role. Requests are signed with Ed25519 over identity, method, path, time and
the body hash (two minutes of clock skew); a replay log (`replay.log`) refuses a repeated nonce, and rates are limited per key and per
source address.

**Rotation.** Removing a person from a tenant or namespace makes an administrator's (or the pod's own) side open all values, make a
new scope key with generation + 1, wrap it for everybody who stays and seal every value again; the service checks that the generation
moves by one, the removed person has no wrap left and no value is lost. A rotation that cannot be done is recorded as pending and
retried (`iyakki cloud reconcile --retry-rotations`).

**Snapshots.** An optional archive encryption (XChaCha20-Poly1305 in 64 KiB chunks bound to position and end of stream); the key is a
file or a `snapshot-key.<id>` secret in the tenant scope.

**Where things are stored.** The service's volume holds `operators.json`, `common/common.json`, and per tenant `roles.json`,
`members.json` and `vault/*.json` (one file per scope, mode 0600, written whole and atomically). Secret **names are in the clear**.

**What an attacker who copies the data volume learns.** Public keys, wrapped scope keys (useless without a member's secret), sealed
values (ciphertext), the secret names, the number of secrets and their approximate sizes, the generation of every scope, the roles
lists and the replay log (identities and nonces). They do **not** learn values or scope keys. What stops them: the member secrets are
not on that volume. What does not: whoever also holds one member's secret opens every scope that member has a wrap in; a removed
member keeps what they already read and any copy of the old ciphertext, since rotation changes keys going forward; names and sizes
leak; there is no forward secrecy for sealed values, only for the one-time wrap keys; a compromised service can withhold, delete or
roll back to an older consistent file (the generation check limits this but there is no signed history).

**Known limits and unreviewed choices.** Hand-written HKDF and HMAC over `sha2` (RFC 5869, RFC 2104, a few lines); member secrets
live as plain hex files; replay-log lines are not fsynced; no key-compromise recovery beyond "enrol a new key and rotate"; no
hardware-backed keys. All of these are exactly what a review would examine.

## 2. The options

The person who runs the system picks the provider with the setting `cloud/shared-vault.json` (`sharedvault.rs`), by hand or with
the chat tool `shared_vault_provider_set` (asks every time).

### (a) Built-in (default): `{"v":1,"provider":"builtin"}`

All of section 1. **Protects:** a copied data volume; a curious operator of the service (it holds ciphertext only); one tenant from
another. **Does not protect:** against a flaw in our construction (not externally reviewed); against a stolen member secret; against
a compromised pod that holds a member's key. Pick it when you have no vault of your own and accept the risk above.

### (b) External vault or KMS: the secret bytes live in your system

```json
{"v":1,"provider":"external","external":{"address":"https://vault.example:8200","mount":"secret","path":"iyakki/acme","token_secret":"shared-vault-token"}}
```

Works with a HashiCorp Vault or OpenBao KV version 2 mount (the same kind the repository already resolves for `ext:ALIAS/path#field`
references in `vault.rs`). IYAKKI keeps **only the reference**: address, mount, path prefix and the **name** of a vault secret that
holds the access token. Secrets are written as `<mount>/data/<path>/<scope>/<name>` with the field `value` (text only). **Protects:**
the secret bytes never touch IYAKKI's volume, snapshots or service; the cryptography, key storage, audit and rotation are your
system's, which you already review and operate. **Does not protect:** the access token still lives in the local vault (whoever opens
it can read the shared secrets while that token is valid, so give it the narrowest policy); the secret names are visible to the
external system and in IYAKKI's audit trail; an unreachable system means no shared secrets (it fails closed, with a plain message,
and never falls back to the built-in store); IYAKKI's per-scope roles are not enforced by your system (one token reads what its policy
allows); the external system's own review status is its own.

### (c) Envelope mode: an external KMS wraps the data key (not built)

The built-in layout stays, but each scope key is wrapped by a KMS key you control (for example Vault Transit, a cloud KMS or an HSM)
instead of by member keys alone. **Would protect:** a copied volume plus member secrets still cannot open anything without the KMS;
revoking the KMS key revokes everything at once; the KMS gives its own audit. **Would not protect:** against our value
construction (the values would still be sealed by our code), against a person who is allowed to call the KMS, or against availability
(no KMS, no vault). It needs a client for the KMS API and a migration of existing scopes. Nothing of this exists yet; it is listed so
that nobody mistakes (b) for it.

## 3. What is implemented (2026-11-06)

* `sharedvault.rs`: the typed setting (`builtin` | `external`), its validation and load/save, the backends, and `SharedVault`, the
  one place that reads and writes shared-vault secrets for a chosen provider, with an audit line per call in
  `cloud/roles-audit.log`: provider, action, scope, secret name, actor and the outcome in plain words; never a value, a token, an
  address or the transport's error text.
* Fail closed: an unreachable or refusing external system, a token secret that is missing, a damaged setting, a newer setting
  (`v` above 1, an unknown provider or field): an error with a plain message, never the built-in store.
* Provider and label (`not externally reviewed`) on Settings, Cloud access (one line), and in the chat tool's answers.
* Chat tools `shared_vault_provider` (read) and `shared_vault_provider_set` (hard-ask, administrators). The setting file is also
  readable and writable by hand. There is no separate web API route for it: the settings page shows it, changes go through the tool.

**Remaining:** the snapshot keyring and the `iyakki vault` command still use the built-in client directly; the built-in service does
not refuse writes by itself when the provider is `external`; the setting is not yet part of the tenant launch configuration
(`cloudtenant.rs`) and no settings-page control changes it (display only); envelope mode (c); a real HashiCorp Vault or OpenBao
has not been tried (the tests use a fake KV v2 server); no migration of secrets between providers.
