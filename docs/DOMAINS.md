# Domain names for services

Give a service a name, for example `store.local` for the private store, instead of an address and a port. Settings, Network, **Domains**
(administrators), or ask the assistant (`domains_list`, `domain_route_add`, `domain_route_remove`, `domains_listener_set`).

## How it works

* A **route** says: this host name (or `*.example.org`, one label), this path prefix, goes to this target (a port on this device, or a
  cluster Service), served as plain http, https with a certificate, or TLS passed through untouched; WebSocket through or not; who may reach it.
  The table is `<data>/domains.json`. The checker is the engine's own (`cloudroute.rs`); its messages are shown unchanged.
* The **listener** (`cloudlisten.rs`) is a small reverse proxy: it reads the `Host` header (or the TLS server name) and sends the request
  to the route's target. It is a service you switch on and off. It starts on **127.0.0.1 only**, on port **8080** (http) and **8443**
  (https). Ports 80 and 443 are used only after you approve them (they need the right to bind low ports); any other port below 1024 is refused.
* **Other computers reach it only after the firewall step**: Settings, Domains, `f` (or `domains_listener_set` with `open_firewall`). It opens
  the listener's TCP ports to the **local network only** (private and link-local addresses, never the internet), optionally until an
  end time. It is a separate approval from switching the listener on (the assistant cannot do both in one call). Switching the listener off
  removes the opening; an end time that passes switches the listener off and leaves a note in the inbox.
* **Local names**: a route host ending in `.local` is announced through the device-name announcer as one more name for this device's
  addresses, while the listener runs. This needs Settings, Network, "Announce this device on the local network" to be on.
* **Certificates** are kept in your vault (`domain.<name>-chain` and `domain.<name>-key`), never in the settings file. `t` makes a trial
  self-signed certificate for the selected route (browsers warn); `i` imports your own PEM files after the engine checks the key belongs
  to the certificate, covers the host and is not expired. The inbox gets a warning 30 days before a certificate runs out, and when it has.
* **Finding services**: adding a route lists the services of the local k3s (`kubectl get svc -A`, through the managed cluster's own
  kubectl, no shell) and this device's own (the private store's NodePort 30580, the web desktop). Picking one fills the port. A NodePort is
  reached at `127.0.0.1:<nodePort>`; a ClusterIP Service is reached through `kubectl port-forward` (one namespace at a time).

## Worked example: `store.local` for the private store (NodePort 30580)

1. Settings, Network, Domains, press `a`.
2. Name `store.local`, path `/`, Send to "Private store (NodePort 30580)" (or the `iyakki-store/iyakki-store-nodeport` entry found in the
   cluster), Served as "Plain http". Save. The page says the listener is off.
3. Press `x`. The status line reads "Listening on port 8080, 8443, on this device only". On this device,
   `http://store.local:8080/` now reaches the store (the device resolves its own `.local` name only if the announcer is on; otherwise use
   `http://127.0.0.1:8080/` with a `Host: store.local` header to test).
4. For another computer, press `f`, choose "Open to the local network", pick how long, confirm. The firewall now allows TCP 8080 from
   the local network. Check Settings, Firewall: a rule "domain listener (closes with its last route)".
5. On the other computer, open `http://store.local:8080/`. Expect: the store's page. If the name does not resolve, the other computer has
   no mDNS (older Windows without Bonjour); use the device address and a hosts-file line instead.
6. To stop: press `x`. The ports close, the firewall opening goes, the name is withdrawn.

Through the assistant: `domain_route_add` (host `store.local`, port 30580), `domains_listener_set` (`enabled`), then, as a separate approval,
`domains_listener_set` (`open_firewall`, optional `expires_hours`).

## Limits, honestly

* "Who may reach it" is recorded, but the listener cannot know who a person is: `members` is **not enforced** by it. The firewall (local
  network only) is the real fence, or put a sign-in in front.
* The listener speaks HTTP/1.1 to targets in plain text; use passthrough for a target that speaks TLS (the web desktop).
* The extra `.local` names are not probed for conflicts with another computer that already uses the name.
* Not tested here: a second computer resolving the name, and a run on a real k3s (tests use fake kubectl output, a fake firewall, a fake
  announcer and a vault in memory; the routing test runs a real listener on loopback). `kubectl port-forward` for ClusterIP Services is untested.
* The listener starts again at sign-in (it needs your vault for certificates); with nobody signed in nothing is served.
