# Running a relay

A relay is a mailbox, not a computer. It lets a computer you work on talk to an IYAKKI machine that cannot be reached directly (behind a
home router, no address to give), and lets fleet members behind NAT reach each other. Both sides make OUTGOING connections to the relay
and exchange small sealed envelopes. Features: F-CORE-17, F-CORE-33, F-FLEET-10. The wire and the sealing are described in
`docs/development/CORE-CONTRACT.md` ("Relay transport"); this page is about running the server, `iyakki-relay`.

## What it can and cannot see

It cannot see: any request, answer, route name, call name, value, key or file. Every body is sealed end to end between the two ends
(XChaCha20-Poly1305 with keys only the paired computer and the machine hold). The relay holds no key and has no code that opens an
envelope; the server reads only the envelope's header.

It can see: which mailbox ids receive envelopes, which sender key id wrote them, when, how big they are, and the network address of
whoever connects. A mailbox id is a hash of a machine's relay key, so it names no person. Treat the log accordingly: the server writes
kinds, sizes and refusal reasons only, never a body, and not a mailbox id.

It does not decide anything: the machine decides (the relay switch is off by default, only a paired computer is answered, the first
contact of a computer is approved on the machine's own screen, the contract's scopes and rate limit apply). A relay that lies, drops or
replays envelopes can make a request fail; it cannot make a machine run one that was not sealed by a paired computer. Replays and
tampered envelopes are dropped by the machine.

Anyone who knows a mailbox id can fetch what waits in it (that is how a mailbox works), which is why the ids are 128-bit hashes and the
contents are sealed. Do not publish them.

## Start it

```
iyakki-relay --cert relay.crt --key relay.key --listen 0.0.0.0:8443
```

* `--cert FILE` the certificate chain, PEM, leaf first. `--key FILE` its private key, PEM. The server refuses a key file that others can
  read (`chmod 600`). The key is read once and never printed or copied.
* `--listen ADDR:PORT` default `0.0.0.0:8443`. Use a port above 1023 so no privileges are needed.
* `--max-wait-ms N` the longest an envelope waits, default and ceiling 86400000 (24 hours). A test relay may set it to a few seconds to
  watch expiry.
* `--mailbox-per-minute N` (120), `--mailbox-depth N` (256), `--peer-per-minute N` (1200 requests per network address), `--max-total-mib N`
  (256), `--max-mailboxes N` (10000), `--max-connections N` (128): the limits. A request over a limit gets a 4xx or 503 with a sentence.
* `--quiet` logs only start and errors.

Queues are in memory: a restart empties them. Requests live at most 24 hours and clients ask again, so nothing durable is lost; run the
relay under a supervisor (a systemd user unit, launchd, a container) so it comes back.

## The certificate

The machine trusts exactly one certificate, by its SHA-256 fingerprint (no certificate authority is consulted), and the name in the URL
must be in the certificate. Make one for the name or address your machines will use, for example with OpenSSL:

```
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 730 -subj "/CN=relay.example" \
  -addext "subjectAltName=DNS:relay.example" -keyout relay.key -out relay.crt
openssl x509 -in relay.crt -noout -fingerprint -sha256
```

On the machine: Settings, Developer, Relay: set the address (`https://relay.example:8443`), paste the fingerprint, switch the relay on.
Then pair or approve the computer on the machine as for developer mode (the first contact raises a question on the machine's screen).
A different certificate is refused ("the certificate changed since you trusted it"), also after a renewal: set the new fingerprint.

## Firewall and network

Open one inbound TCP port on the relay host (the one in `--listen`); nothing else is needed, and nothing is opened on the machines. For
a trial keep it on the local network only: do not forward the port from a router to the internet unless you mean to run a public relay,
and then set `--peer-per-minute`, `--max-total-mib` and `--max-mailboxes` to what the host can bear and put it behind your usual
protections. The server speaks HTTPS only (no plain port).

## The wire

* `POST /v1/relay/post`, body one encoded envelope (Content-Length required, at most 64 KiB): 204, or 400 (not an envelope, wrong version,
  ttl outside 1 ms to 24 h, sender clock more than 5 minutes ahead), 410 (already expired), 413, 429 (mailbox or address rate, mailbox
  full), 503 (relay full).
* `GET /v1/relay/fetch/MAILBOX`: 200 and a JSON list of hex envelopes (at most 64), each handed over once, oldest first. Expired ones are
  dropped, never delivered.
* `GET /healthz`: `ok`.

## Stop it

Send the process a termination signal (`kill PID`, or stop the service). Nothing needs saving.

## Tests

`cargo test -p iyakki-relay` (queues, limits, expiry with a manual clock, the HTTP handling over in-memory streams) and
`cargo test -p iyakki-agent --lib devmode::tests_relay` (the real server on loopback over TLS with a pinned certificate: first contact
needs approval, a round trip, expiry, replayed and tampered envelopes, a wrong certificate). A run against a relay on another computer on
the local network was done by hand for the alpha.7 line; see the row notes of F-CORE-33.
