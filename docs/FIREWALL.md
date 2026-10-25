# Firewall

IYAKKI blocks every new incoming connection by default. You allow what you want: a host name, an IP address, an IP range
or a network (CIDR), with optional ports, protocol, interface and an end time, or switch on a one-click preset. Changes are
applied with a 60-second "Keep these rules?" so a wrong rule cannot lock you out for good.

Settings -> Firewall (administrators; the page is also there on cloud machines) opens on a **simple view**; everything else is one
fold away under **Advanced**. Nothing pretends: if the rules cannot be put in the kernel the page says NOT PROTECTED and why.

Contents: [The simple view](#the-simple-view) · [What it does](#what-it-does) · [Mechanism research and decision](#mechanism-research-and-decision) ·
[The rules](#the-rules) · [Presets](#presets) · [Host names and DNS](#host-names-and-dns) · [Applying safely, and getting out of a lockout](#applying-safely-and-getting-out-of-a-lockout) ·
[Blocked connections](#blocked-connections) · [Files, boot and audit](#files-boot-and-audit) · [Limits and planned](#limits-and-planned) · [Tests](#tests)

## The simple view

What most people need, in plain words and with three kinds of control only.

* **How strict?** Three choices, the current one highlighted (keys `1`, `2`, `3`, or a click):
  * **Standard**: block everything coming in, allow what you open below. This is the default and the recommended one.
  * **Open**: no blocking. Choosing it asks first ("Anything on this computer that listens can be reached from the network"). It
    takes effect at once (switching the firewall off cannot lock anyone out); Standard again asks to be kept, like any change.
  * **Locked down**: also block outgoing connections except what you allow. **Not available yet**: this firewall only filters what
    comes in (see "Limits and planned"). The card is there so the promise is visible, and choosing it says so instead of
    pretending; it never changes anything.
* **Allowed on this computer**: a short list, one line each, with big **Add...** and **Remove**. Add offers a list of services:
  Web server (80, 443), SSH (22), File sharing (shared folders), Remote desktop, Kubernetes / k3s API (6443), **Custom port...**
  (a port or range, TCP/UDP, and who may connect: the local network, or anyone) and **A computer by name or address...** (paste a
  host name, an IP address, a range `a-b` or a network `a/len`; the kind is recognised for you). Services are the presets of
  Advanced; a custom port is one rule per local network, shown as one line and removed as one line.
* **Safety.** Every change from the simple view is applied at once and then waits for **Keep** (or **Undo**) for 60 seconds, exactly
  like the Advanced page: if nothing is pressed, it is undone. Removing **SSH** or **Remote desktop** asks first, because that may be
  how you are connected ("you will lose the connection... undone by itself after 60 seconds unless you choose Keep"), so a mistake
  there gets you back in. Open asks first. Locked down refuses.
* The big state (FIREWALL ON, OPEN, NOT PROTECTED) and its sentence stay at the top; the blocked-attempts count is one line at the bottom.

**Advanced...** (key `A`, then **Simple view** brings you back) is the page as it was: every rule with its type, ports, interface and end
time, presets and ping, host-name refresh, the blocked-connection log, import/export, Disable now. The engine, its default
(block incoming) and its tests are the same underneath; the simple view only edits the same settings. Changes made in
Advanced and not applied yet are shown in the simple view too, with Apply and Discard.

## What it does

* **Incoming**: policy drop. Only these get in without a rule of yours:
  * replies to connections this machine started (established and related), so browsing, updates, DNS lookups and downloads work;
  * traffic on the loopback interface (`lo`): a service and its client on the same machine always work;
  * the cluster and container networks of this machine (`cni*`, `flannel*`, `veth*`, `docker*`, `podman*`, `cali*`, `cilium*`,
    `kube-*`): pods reach the host's API server and DNS. Switchable (Presets and basics), on by default;
  * DHCP replies (UDP 68), IGMP, and when IPv6 is on: neighbour discovery and multicast listener messages (ICMPv6 130 to 137, 143)
    and DHCPv6 replies (UDP 546 from `fe80::/10`);
  * **ping** (echo request) from the local network only, by default. Switchable to "not answered" or "from anywhere";
  * the **Remote desktop / web mode** preset (TCP 8443, or the port given with `iyakki.web=`), on by default, from the local network.
    On a cloud machine (`iyakki.mode=cloud` or a cloud image) it is open to everyone, because a browser is the only way in; the page says so.
* **"The local network"** means the private and link-local ranges: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
  `169.254.0.0/16`, and for IPv6 `fe80::/10` and `fc00::/7`. A network that uses public addresses internally needs explicit rules.
* **Outgoing**: not filtered in this version (planned).
* **Forwarding**: not touched. The firewall has one chain, on the INPUT hook. It does not forward, NAT or mark anything.
* Broken packets (`ct state invalid`) are dropped first.

## Mechanism research and decision

What the images have today (read from the real kernel configs of the built images, `target/buildroot-web` and `target/buildroot-mac`,
kernel 6.12.112, and from `dist/buildroot/board/iyakki/linux.fragment`; the aarch64 fragment asks for the same options):

| Option | In the image kernel |
|---|---|
| `CONFIG_NETFILTER`, `NETFILTER_NETLINK`, `NETFILTER_NETLINK_LOG` (nflog), `NF_CONNTRACK`, `NF_CONNTRACK_FTP` | built in (`=y`) |
| `CONFIG_NF_TABLES` | module (`nf_tables.ko`) |
| `CONFIG_NF_TABLES_INET` (and IPV4, IPV6) | built in to `nf_tables.ko` (the `inet` family) |
| `NFT_CT`, `NFT_LIMIT`, `NFT_LOG`, `NFT_REJECT`, `NFT_REJECT_INET`, `NFT_NAT`, `NFT_COMPAT` | modules |
| counter, meta, payload, cmp, bitwise, lookup, immediate | part of `nf_tables.ko` (the kernel has no `NFT_COUNTER` option any more) |
| `IP_NF_IPTABLES` (legacy), `IP_NF_FILTER`, `IP6_NF_FILTER`, `NETFILTER_XT_MATCH_CONNTRACK` | built in |
| the `nft` or `iptables` **programs** | not in the images (only the optional cluster variant lists iptables) |

So the kernel can do it and **no kernel rebuild is needed**: the modules are already in `/lib/modules/6.12.112/kernel/net/netfilter/`.
The firewall loads `nf_tables`, `nft_ct`, `nft_limit` and `nft_log` with `modprobe` before the first rule (`crates/iyakki-fw/src/backend.rs`).
If the kernel has no nf_tables at all, the page says **"Firewall not available in this kernel"**, in red, and that incoming connections are not blocked. There is no software fallback.
`crates/iyakki-fw/tests/kernel_options.rs` checks that both kernel fragments keep asking for these options.

**Decision: (a), our own nf_tables writer over netlink** (`crates/iyakki-fw/src/nl.rs`), not the `nftables` package.

| | (a) in-house netlink writer | (b) Buildroot `nftables` |
|---|---|---|
| New program in the image | none | `nft` + libnftnl + libmnl + gmp (about 1 to 2 MB) |
| Licence | ours (Apache-2.0) | GPL-2.0 program, LGPL libraries: a new row in LICENSING and source offers |
| Injection risk | none: the rules are typed steps (numbers), no text is ever parsed | a text ruleset: every value must be quoted and escaped |
| Effort | about 600 lines (messages for table, chain, rule, and 12 expression kinds) | small |
| Risk | the encoding must match the kernel | low |

The deciding points were the standing rule to prefer our own code over dependencies (permissive licences only), no new program at the
base-OS boundary, and no text format to get wrong. The risk of (a), a wrong encoding, was removed by **running it against a real kernel**: the
test in `crates/iyakki-fw/tests/kernel.rs` (ignored by default, runs under `unshare -rn` so the machine's own network and firewall are never touched)
loads the ruleset, has the stock `nft` list it back, connects from a second
network namespace to show that an allowed range gets in and another port is dropped, and reads the counter and the blocked-attempt log.

How it fits with k3s and everything else: our table is **`inet iyakki_fw`**, one base chain `input` (hook input, priority 0, policy drop). Every apply is one atomic batch:
create the table if missing, delete it, create it again with the new chain and rules, so a half-applied ruleset can never be seen by the kernel. Nothing else
is touched and the ruleset is never flushed: k3s, kube-proxy, flannel and the CNI keep their own iptables rules (they use the kernel's iptables compatibility with their own tables).
A packet dropped by any table's INPUT chain is dropped, so our policy does apply on top of theirs; the cluster interfaces are trusted for that reason.

## The rules

Rules only **allow**, so their order does not matter. Each has:

| Field | Meaning |
|---|---|
| Type | **Hostname**, **IP** (v4 or v6), **IP range** (`192.168.1.10-192.168.1.50`), **CIDR** (`192.168.1.0/24`, `2001:db8::/48`) |
| Value | what the type needs; checked as you type it |
| Protocol, Ports | TCP+UDP, TCP or UDP; ports empty (every port), `22`, `80,443`, `8000-8100` (up to 16 items) |
| Interface | optional: only packets that arrive on this interface (`eth0`) |
| Note | for you, up to 80 characters; never sent to the kernel |
| Allow for | never, 1 hour, 1 day; a rule that has ended stops working by itself and shows "ended" |
| On/off | the "Allow on/off" key switches a rule without deleting it |

Checks (plain messages): not an IP address; reversed range; both ends of a range must be the same family; `10.0.0.5/24` has host bits set (the network is `10.0.0.0/24`);
the CIDR length is at most 32 or 128; `/0` is accepted with the warning "allows everyone on the internet in"; very large ranges and networks are warned about;
ports are 1 to 65535; `0.0.0.0`, `::`, multicast and broadcast are refused; names follow DNS syntax (below). Nothing typed can become part of a rule's
meaning: the generator only ever sees numbers, so spaces, semicolons, quotes and line breaks in a name or a note cannot inject anything (names with them are refused; a note may contain
quotes but no control characters, and it never leaves the page).

The page lists the rules in a table (type and value, protocol and ports, interface, end, the addresses a name resolved to, and a status: allowed, off, ended, no address, invalid),
the presets, the state, the number of blocked attempts and the last ones. **Import / export**: "Copy rules" puts the settings on the clipboard as JSON; "Import" reads them back as
unapplied changes (a bad or hand-edited export is refused with the reason and changes nothing).

## Presets

"Presets..." is one form of on/off choices with an explanation under each. All are "from the local network" unless said.

| Preset | Allows | Default |
|---|---|---|
| SSH | TCP 22 | off |
| Remote desktop / web mode | TCP 8443 (and the port in `iyakki.web=`); from anywhere on a cloud machine | **on** |
| Shared folders | TCP 8081-8180, where Shared folders picks its ports | off |
| Kubernetes API | TCP 6443 | off |
| Kubernetes NodePorts | TCP and UDP 30000-32767 | off |
| Private store NodePort | TCP 30300 (this machine itself always reaches it: loopback is always allowed) | off |
| Allow DNS service | TCP and UDP 53 to this machine, for a machine that runs a DNS server | off |
| Web server | TCP 80 and 443 (the simple view's "Web server") | off |

* **Remote diagnostics.** The boot entry "remote diagnostics" (`iyakki.ssh=1`) always keeps SSH reachable: with that parameter on the command line the firewall allows TCP 22 from the local network and the page says so.
* **Shared folders.** A share started for "anyone on the local network" or with a secret link is blocked from other devices until the Shared folders preset is on. The Shared folders page shows a note in that case
  and `f` switches the preset on (applied at once; you keep or revert it on the Firewall page).
* **"Allow DNS" has two meanings and both are there, clearly labelled.** (1) *Allow by DNS name*: the Hostname rule type. (2) *Allow DNS service (port 53) to this machine*: the preset above, for machines that run a DNS server.
  Looking names up from this machine always works, with no rule, because the answers are replies to connections it started.
* Ping is not a preset but a setting in the same form: not answered, from the local network only (default), from anywhere.

## Host names and DNS

IYAKKI looks each name up when it is added, then every N minutes (default 5, 1 to 1440) and whenever the network changes, and writes the addresses it found (up to 16, IPv4 and IPv6) into the rules.
The addresses are shown on the rule. If a lookup fails the last good addresses stay and the page shows a warning; a name that has never resolved allows nothing and says "no address". The last good answers are saved, so the next
boot applies them before the network is up. A removed rule's name is forgotten.

Caveats, also on the page:
* **A name is only as trustworthy as the DNS server that answers for it.** Someone who controls your DNS (or your Wi-Fi's DNS) can make a name point at their computer and so walk through the firewall. For anything that matters, use an IP address or a network.
* Wildcard names (`*.example.com`) are not supported; a name is one exact host.
* A name with many changing addresses (a CDN) works only as well as the 16 addresses seen at the last lookup.
* Names are ASCII; write international names in `xn--` form. Underscores are not allowed. Trailing dots are removed and case does not matter.
* Lookups run in a separate thread and never hold up the page.

## Direct links (one cable between two computers)

An interface that the link manager brought up for a Thunderbolt/USB4 cable, a host-to-host USB bridge cable or a USB gadget port
(`docs/DEVELOPER-MODE.md`, section 10) is a **direct link**: one cable, one other computer, no router. The generator puts a zone for each such
interface right after the base rules (replies, loopback, containers, DHCP, IPv6 neighbour discovery) and before ping, presets and your rules:

| On the direct link | Default | With "Allow more on direct links" |
|---|---|---|
| TCP 7979 (developer mode) | allowed | allowed |
| UDP 7980 (the peer beacon) | allowed | allowed |
| ICMP and ICMPv6 | allowed | allowed |
| everything else | **dropped**, even where a preset or rule would allow "the local network" (a peer's link-local address counts as local, so without the final drop the remote-desktop preset would have opened port 8443 on the cable) | allowed (presets and rules apply too) |

Nothing is routed or translated between the cable and other networks: the firewall has only an input chain, and the link manager turns forwarding off on the interface.
The switch is in Settings -> Developer mode -> Connect over USB-C (`m`) and `iyakki dev usb link --allow-more on|off`; it is kept in `<data>/directlink.json`,
not in the firewall page's keep-or-revert flow. Without a direct link no rule is added. Interface names are checked before they go into a rule. Tests: `iyakki-fw`
`a_direct_link_lets_in_only_developer_mode_the_beacon_and_ping_by_default` and two more. Not loaded into a real kernel with a real cable.

## Applying safely, and getting out of a lockout

Edits are a **draft** ("not applied" on the rule). **Apply (y)** puts the whole change in the kernel at once and starts a **60-second confirmation**:
*Keep these rules?* **Keep (k)** saves them; **Revert (v)** or doing nothing takes them back to the last rules you kept. This holds when you are
using the remote desktop, the web mode or SSH: if the change cuts your session off you cannot press Keep, and after 60 seconds the old rules are back and the session works again.
Turning the firewall off needs no confirmation (it cannot lock anyone out). Nothing is saved until Keep, so a restart during the 60 seconds also returns to the old rules.

If you are locked out anyway (a rule you kept that you should not have), in order of ease:

1. **Devices list (F8)**: the Firewall row has an **Off** button (administrators). That is the shell's emergency key; it removes the rules at once and keeps the firewall off until you switch it on again.
2. **Settings -> Firewall -> Disable now (z)**.
3. **From a console or SSH**: `iyakki firewall off` removes the rules, saves "off" and tells the running firewall; `iyakki firewall status` shows the settings and whether the kernel table is in force.
4. **Boot parameter** `iyakki.firewall=off` (edit the boot entry): skips the rules for that one boot, and the page says "OFF for this boot only". The next boot applies the saved settings again.
5. Last resort: delete `/var/lib/iyakki/firewall.json` (the safe defaults are then used: everything incoming blocked, web mode open to the local network).

Status words: **ON**; **OFF** (switched off, or off this boot); **NOT PROTECTED** in red when the rules could not be applied (the kernel refused them, the settings could not be applied at boot, a revert failed) with the reason,
or "Firewall not available in this kernel". A failed change leaves the previous rules in force (the kernel applies a batch all or nothing) and says that the change was not applied. When the firewall is wanted but not in force, the Devices button in the top bar gets a red dot and its Firewall row says NOT PROTECTED.

## Blocked connections

A rule at the end of the chain counts new connections addressed to this machine that no rule allowed (broadcast and multicast noise is not counted), and a second one, limited to 10 per second, sends a copy of the first bytes to an nflog group. IYAKKI reads that group
into a ring of the last 50 attempts in memory (source, protocol, port, how long ago; the same client retrying is one line with a count). Nothing goes to the kernel log or to the disk, so a port scan cannot fill either. "Clear log" empties the list.
The count and the list start empty at each boot.

## Files, boot and audit

* Settings: `/var/lib/iyakki/firewall.json` (written atomically, administrators' machine settings). The file holds the rules, the presets, the ping choice, the trust of container networks, the look-up interval and the last good addresses of names. Unreadable files are set aside as `.bad` and the safe defaults are used.
* Boot: `late_boot` in `crates/iyakki-core/src/boot.rs` applies the firewall **first**, before the network is configured and before the SSH server and any other service listens, and again once the network is up (names are looked up then).
* Audit log (`audit.log`): `firewall.rule.add|edit|delete|toggle`, `firewall.presets`, `firewall.preset`, `firewall.import`, `firewall.apply`, `firewall.keep`, `firewall.revert`, `firewall.disable`, and from the engine itself (user "system") `firewall.boot`, `firewall.boot_skipped`, `firewall.apply_failed`, `firewall.dns`, `firewall.expire`, `firewall.network`, `firewall.settings_unreadable`.
* A window of the shell on someone else's computer shows the page but changes nothing there.
* The new kernel command-line option `iyakki.firewall=off` is listed in `BOUNDARY.md`.

## Limits and planned

* Outgoing connections are not filtered (planned). Forwarding is not filtered.
* The rules are written one per combination of source, protocol and port (a limit of 3000 rules, with a message that says how to use fewer); nf_tables sets would make large lists shorter (planned).
* IPv6 extension headers before the transport header are handled by the kernel's header search, not by us.
* The SSH preset only helps when an SSH server runs. This machine has none unless the boot entry `iyakki.ssh=1` was used.
* A multi-node dev cluster needs more: flannel's VXLAN (UDP 8472) and the kubelet (TCP 10250) between the nodes, as rules for the other nodes' addresses. A single-node cluster (the default) needs nothing: its traffic is on the trusted cluster interfaces.
* Host-network pods and `hostPort`s of the dev cluster that other computers should reach need a rule or the NodePort preset.
* Tailscale and other VPN address ranges (`100.64.0.0/10`) are not "the local network"; add them as CIDR rules.
* The "Firewall" page is in Settings (and reachable from the Devices list); a tile in the System group would need a new app entry in the permissions table and was not added.

## Tests

* `cargo test -p iyakki-fw`: ruleset generation (golden files in `crates/iyakki-fw/tests/golden`: default, minimal, ping modes, each rule type, v4/v6, ports and protocols, interface, host names with a fake resolver, presets, the `iyakki.ssh=1` case, cloud); rule evaluation (what gets in and what does not); validation and injection attempts; the netlink messages byte for byte; the apply / keep / revert state machine with a fake clock and fake backend; failure and "NOT PROTECTED"; boot skip; DNS refresh with changing and failing answers; expiry; persistence; the blocked log.
* `cargo test -p iyakki-shell firewall_tests`: the simple view (modes, Add, Remove, custom port and address, the questions, the 60-second undo, drawn inside the window at five sizes) and the Advanced page, the draft, Apply/Keep/Revert, audit entries, forms, import/export, shares note, rendering at 360x740, 800x480, 1280x720 and 1920x1080 in every state.
* `cargo test -p iyakki-core --lib boot::tests::the_firewall_is_applied_first_in_late_boot_and_again_once_the_network_is_up`.
* `cargo test -p iyakki-fw --test kernel -- --ignored`: the real kernel in a private network namespace (needs `unshare`, `nft`, `python3`, `ip`, `nsenter`; skipped with a note when missing).
* Not yet proved on a device: boot order on real hardware, k3s running together with the firewall, the Devices button red dot on screen, modules loading at boot on the Mac mini, behaviour with Wi-Fi roaming.
