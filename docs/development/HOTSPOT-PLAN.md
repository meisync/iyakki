# Wi-Fi hotspot: share the internet through a chosen Wi-Fi card

Status: **plan and hardware-free core written; wired, USB and Thunderbolt internet sharing built (2026-11-04, owner order: wired first, Wi-Fi afterwards), proved by tests with fakes and loopback sockets, not yet on real hardware**. Sharing code: `crates/iyakki-agent/src/sharing/` (manager, plan, firewall table, sockets, service, `iyakki share`), page `crates/iyakki-shell/src/apps/settings_share.rs` (matrix rows `F-HOTSPOT-10` to `-17`). The Wi-Fi access point and Bluetooth are not started yet; both are modelled as kinds that answer "not available on this build". Requested by the owner on 2026-11-04: "Provide option for sharing internet over selected device,
tethering or hotspot". Phase P1 of the Wi-Fi access point (real nl80211 transport, capability probe, bring-up on virtual radios) is built and passes the virtual-radio test (see the roadmap). Nothing here is switched on in the product yet. The pure code is in `crates/iyakki-hotspot` (matrix rows `F-HOTSPOT-01` to `-12`).

Owner decisions of 2026-11-04:

* The **first version is the Wi-Fi hotspot only**: a Wi-Fi card that the person selects becomes an access point and shares the machine's internet that arrives on
  another link (Ethernet, another Wi-Fi card, a USB or phone link). Wired and USB sharing ("tethering") come later.
* The access point is **our own code**. No `hostapd` on the image. Rule of the project: prefer own code, permissive licences only, no copying of code without a
  licence. Test vectors from the public IEEE 802.11i, RFC 6070 and RFC 2104 documents are fine; the constants of `linux/nl80211.h` are facts of the kernel interface.

## Goals

1. A person opens Settings, Network, **Share internet**, picks the Wi-Fi card and the link that has the internet, and presses Start. Phones and laptops can then
   join the network, get an address and use the internet through this computer.
2. Safe by default: a strong generated passphrase, a name that does not identify the person, a small number of devices, a switch-off after idle time, and no way
   for a joined device to reach anything on the computer except the two services it needs (address and name lookup).
3. Small and in-house: DHCP, DNS forwarder, handshake and access point control are written here, with no new dependency and no `unsafe` in the core crate.
4. Honest about hardware: the pure parts are tested completely on any machine; the radio, the driver and the firmware can only be proved on a real card, and the
   plan says exactly which parts those are.

Not goals of the first version: WPA3 (SAE), 5 GHz DFS channels, 802.11n/ac/ax rates (the first version advertises legacy rates and lets the driver and firmware do
the rest; speed is a later step), IPv6 on the hotspot, bridging, captive portal, QR code (planned for the page, not the core), Wi-Fi direct, wired and USB sharing.

## Generic principle (owner decision 2026-11-04)

**The hotspot is generic: no per-chip and no per-driver code, anywhere.** The access point is configured on whatever Wi-Fi interface the kernel's
cfg80211/nl80211 already exposes. If a driver is already there, we only configure it to run as an access point. Nothing in the product names a chip,
a vendor or a USB id, and nothing ever needs a dongle: a particular dongle is only the recommended device for the *real-hardware acceptance test*
(P2), never a dependency.

How that is kept honest:

1. **A pure capability probe** (`iyakki-hotspot::caps`): `NL80211_CMD_GET_WIPHY` with the split-dump flag is parsed into a `PhyCaps` struct (interface
   types, valid interface combinations with their limits and channel counts, bands and channels with the flags disabled / no-IR / radar / indoor-only /
   no-HT40 / no-20 MHz, cipher suites, maximum AP stations, feature flags and extended features, self-managed regulatory domain, driver-side AP SME).
   The regulatory country comes from `NL80211_CMD_GET_REG`. The driver name is read from sysfs *for the sentence shown to a person only*, never for behaviour.
2. **One pure decision**, `can_host_ap(caps, Want) -> Ok(ApPlan) | Err(Refusal)`: it picks the 20 MHz channel (2.4 GHz prefers 6, 1, 11; 5 GHz the first
   channel without radar detection), says who runs the handshake (`ApMode`), and gives **one plain line for every "no"** plus the fix a person can apply.
   The Settings page and the sharing manager call it to list the cards that can host a hotspot and to explain why the others cannot.
3. **Who runs the handshake follows the capabilities, not the chip**: `Software` (SoftMAC / mac80211: our `mgmt` and `handshake` code, the normal case),
   `PskOffload` (the card advertises `4WAY_HANDSHAKE_AP_PSK`: hand it the PSK, skip our authenticator; not built yet) and `CardManaged` (the driver does
   authentication and association itself, `NL80211_ATTR_DEVICE_AP_SME` present, FullMAC style: "handled by the card"; not built yet, refused with that sentence).
   Note: `NL80211_ATTR_WIPHY_SELF_MANAGED_REG` is **not** a FullMAC marker (several SoftMAC Intel cards set it because firmware owns the country); it only
   means the country cannot be set from here.

The one-line reasons (all produced by the code, each covered by a fixture test): no wireless interface or a Wi-Fi device without driver or firmware
(for example the Mac mini's Broadcom without Apple firmware: the device is on the bus, no radio exists); the card has no AP mode; no CCMP; the radio cannot be
an access point and a client together ("use another uplink or another card"); the client sits on a channel where an AP is not allowed; no usable channel until
a country is set (offer: set the country); the firmware decides the country rules; only radar-checked channels are open here; no such band.

### Limits that no software can remove

* **No firmware** for the card: the interface never appears. The image's firmware pinning must carry the blob (firmware by button, release plan).
* **No AP mode in the driver** (`NL80211_IFTYPE_AP` absent): many STA-only chipsets.
* **Single-radio concurrency limits**: most cards allow AP + client only on one shared channel, some not at all; a single radio used as the uplink cannot also host
  unless the combination table says so (the probe reads it; the page says "use another uplink or another card").
* **Regulatory restrictions**: a no-IR channel cannot start an AP until a regulatory domain permits it; DFS channels need radar detection; some firmware
  (self-managed regulatory) refuses whatever we set. The country is user/machine data (never guessed from the network).

## Threat model

| Who or what | What we do |
|---|---|
| A stranger in radio range who does not know the passphrase | WPA2-PSK with CCMP only (no open network, no TKIP, no WEP, no WPS). The passphrase is generated by default: 16 characters from a 32-symbol alphabet (80 bits) from the operating system's random source (`getrandom`). A typed passphrase must have 8 to 63 printable ASCII characters, and a hint is shown for short or obvious ones. |
| A stranger who records a handshake and guesses offline | The 80-bit default makes that infeasible; a typed short passphrase is warned about. PBKDF2 uses 4096 rounds as the standard says (cannot be changed). WPA3-SAE removes this attack; it is a later step (needs elliptic curve code, P-256 and hash-to-curve, and a driver that accepts SAE; planned, not started). |
| A device that joined (a guest) trying to reach the computer | Everything to the hotspot interface is dropped by the firewall except UDP 67 (DHCP) and UDP 53 (DNS) to the hotspot address only. No SSH, no developer mode, no web service, no mDNS, no ping answered beyond what the firewall's base rules already allow (ICMP echo is a firewall setting; the hotspot does not add one). The hotspot does not run any other server. |
| A guest attacking other guests | **Client isolation on by default**: the driver option (`NL80211_ATTR_AP_ISOLATE`) and a forward rule that drops traffic from the hotspot interface to itself. Can be switched off by an administrator for a person who wants a shared printer. |
| A guest reaching the home network (the uplink's LAN) | Forwarding goes only from the hotspot interface to the chosen uplink, masqueraded. A forward rule drops traffic to the uplink's private ranges (the LAN) by default; "Let devices reach my local network" is a separate switch, off, administrators only. Never bridging: the hotspot is always its own routed /24, so broadcast storms, DHCP servers and ARP of the LAN are never shared. |
| A guest flooding the DHCP pool | Only stations that finished the 4-way handshake are served; each holds at most one address. Unconfirmed offers expire in 30 seconds. |
| A guest flooding the DNS forwarder | Only the hotspot subnet is served; 512-byte query limit; a bounded table of pending queries (answers matched by id and question); the forwarder never answers anything but its own name from the cache because it keeps no cache. |
| Too many devices | A maximum number of devices (default 8, at most 32), checked at association (status "AP unable to handle additional stations"). |
| Forgotten hotspot | **Off by default**, off after every boot, and switched off by itself after N hours (default 2, 1 to 72) with no client online. A notice shows on the Settings page and in the shell status area while it is on. |
| Someone changing it without being an administrator | Starting, stopping and changing the settings need the Network permission that administrators hold (same rule as the firewall page); every start, stop, join and leave is written to the audit log (device address and name, never the passphrase). The passphrase is stored in the personal/machine vault, not in the plain settings file. |
| A forged handshake frame | MIC is checked with the key derived from the passphrase; a frame with a wrong MIC is ignored (anyone can send one); the replay counter must match the latest transmission; the client's RSN element must be exactly CCMP and PSK; the pairwise key is installed only after message 4 proves the client holds it. |
| Deauthentication and other management frame forgery | Not solved by WPA2: protected management frames (802.11w) are optional and a later step. A forger can disconnect a client but cannot join or read traffic. |
| Region rules | The kernel's regulatory domain decides allowed channels and power. The first version uses 2.4 GHz channel 1, 6 or 11 only (no radar detection needed, allowed everywhere); the regulatory country comes from the machine's region setting (needed before 5 GHz is offered). If the driver refuses to start the AP (no-IR flag), the page says why. |

## Architecture

```
 Settings page ──► hotspot manager (iyakki-agent, later) ──► iyakki-hotspot (pure core, this crate)
                       │                                       ├ settings   names, passphrase, limits
                       │                                       ├ subnet     choose a free /24
                       │                                       ├ kdf        PBKDF2, PRF, PTK        (iyakki-sec: HMAC-SHA1)
                       │                                       ├ aes, keywrap  AES-128/192/256, RFC 3394
                       │                                       ├ eapol, handshake  4-way handshake, authenticator
                       │                                       ├ mgmt       auth / assoc / beacon / probe / deauth frames
                       │                                       ├ nl80211    message builders, parsers, ApControl<transport>
                       │                                       ├ dhcp       DHCP server state machine
                       │                                       └ dns        forwarder packet handling
                       ├─► real netlink transport (needs unsafe; separate small crate, hardware step)
                       ├─► UDP sockets for DHCP (67) and DNS (53) bound to the hotspot interface only
                       └─► firewall: iyakki-fw nl::tablegen rules (hotspot zone, forward, masquerade)
```

### 1. Access point control with nl80211

Generic netlink, family `nl80211`, written by hand like the firewall's nf_tables writer (`crates/iyakki-fw/src/nl`). The core builds and parses the bytes as pure
functions with golden tests; a `NetlinkTransport` trait sends them. Calls, in order:

1. `CTRL_CMD_GETFAMILY` ("nl80211") for the family id; `NL80211_CMD_GET_WIPHY` dump to list the cards. A card is **eligible** only if
   `NL80211_ATTR_SUPPORTED_IFTYPES` contains `NL80211_IFTYPE_AP`. The page lists only such cards and says why others are missing.
2. The interface must be down and not in use as a client. `NL80211_CMD_SET_INTERFACE` to `AP`. If the client side (wpa_supplicant) is using it, the manager stops
   the client first (the client is not allowed to join networks on that card meanwhile; the page says so). Concurrent STA and AP on one card (a "combination" in
   `NL80211_ATTR_INTERFACE_COMBINATIONS`) is **optional**: many cards allow it only on the same channel as the client. The first version does not rely on it:
   it uses a **card that is not the uplink** (if the uplink is Wi-Fi, the person must have two cards). Reading combinations to allow one-card repeating is a later step.
3. `NL80211_CMD_START_AP` with the beacon head and tail we build (`mgmt::beacon_head`, `beacon_tail`; the kernel adds the TIM), the probe response template, SSID,
   interval 100 TU, DTIM 2, WPA2, CCMP, PSK, open-system authentication, privacy, the channel (20 MHz) and an inactivity timeout.
4. The kernel and driver (or the firmware, on "softmac"/"fullmac" cards) run the radio and the **CCMP datapath**. We never encrypt data frames. We only supply keys.
5. **Authentication and association** (cards in the common mac80211 mode do not do this for us): `NL80211_CMD_REGISTER_FRAME` for authentication (0x00b0) and
   association request (0x0000) frames; received through `NL80211_CMD_FRAME` events; answered with `NL80211_CMD_FRAME` (open-system authentication, association
   response with an association id). Cards that handle these in firmware ("fullmac" with AP SME) are told apart by the driver refusing the registration; the first version
   supports mac80211-style cards and says "not supported" for others. `mgmt::auth_response`, `parse_assoc_request`, `assoc_status` and `assoc_response` do the content.
6. `NL80211_CMD_NEW_STATION` after a good association (flags authenticated and associated, not authorized, so data frames wait).
7. **EAPOL** frames (ether type 0x888e) travel either through the nl80211 control port (`NL80211_CMD_CONTROL_PORT_FRAME`, kernel 5.x and later; registered with
   `NL80211_ATTR_CONTROL_PORT_ETHERTYPE`) or through an `AF_PACKET` socket on the AP interface. The first choice is the control port; the second is the fallback when
   the kernel or driver refuses it. Both are in the hardware step.
8. When the handshake ends well: `NL80211_CMD_NEW_KEY` (pairwise, CCMP, key index 0, the client's address), then `NL80211_CMD_SET_STATION` with the **authorized**
   flag. The group key goes in once at the start with `NL80211_CMD_NEW_KEY` (no address) and `NL80211_CMD_SET_KEY` (default multicast key).
9. Leaving, idle time-out or a failed handshake: `NL80211_CMD_DEL_STATION` with a de-authentication reason (15 for handshake time-out). Stop: `NL80211_CMD_STOP_AP`,
   then `SET_INTERFACE` back to station mode, and the client side may start again.

The real socket needs `unsafe` (system calls). The core crate is `#![forbid(unsafe_code)]`, so the socket lives in `crates/iyakki-agent/src/nl80211_os.rs` (built 2026-11-04, phase P1) and
implements `NetlinkTransport`, in the same way `iyakki-fw` holds its netlink writer (that one is `NETLINK_NETFILTER`-specific, so its helpers did not fit and a generic-netlink client was written).
`UnsupportedTransport` remains the answer on builds without it. Proved on virtual radios, see "What the virtual radios showed".

### 2. WPA2-PSK handshake (own code)

* PMK = PBKDF2-HMAC-SHA1(passphrase, SSID, 4096 rounds, 32 bytes). PBKDF2 is ours (`kdf`), HMAC-SHA1 comes from `iyakki-sec`.
* 4-way handshake as the authenticator (`handshake::Authenticator`), CCMP, key descriptor version 2:
  M1 (ANonce, ack) → M2 (SNonce, RSN element, MIC) → M3 (install, secure, MIC, key data = RSN element + GTK KDE, AES key wrapped with the KEK) → M4 (MIC).
  PTK = PRF-384(PMK, "Pairwise key expansion", min/max of addresses and nonces) cut into KCK 16, KEK 16, TK 16. MIC = first 16 bytes of HMAC-SHA1(KCK, frame with MIC zero).
* AES-128 (and 192, 256 for the test vectors) in `aes`, RFC 3394 key wrap in `keywrap`. No AES existed in the repository.
* Replay counter goes up with every transmission; a reply must carry the latest. Retries: 4 transmissions, 1 second apart; then the client is removed. Bad MICs are
  counted: a handshake that times out after bad MICs is reported as **wrong passphrase** (the page says "a device tried to join with the wrong passphrase", with its
  address, never the attempt).
* Group key rotation when a client leaves is a later step (not needed for a hotspot of known devices); the group key is new at every start.
* The state machine takes the time as a number and returns actions (send, install key, authorized, failed). The caller owns timers and sockets.

### 3. DHCP server (RFC 2131 subset)

`dhcp::DhcpServer`, UDP 67 bound to the hotspot interface only. DISCOVER/OFFER/REQUEST/ACK/NAK/RELEASE/INFORM (and DECLINE with a ten-minute quarantine of the
address). Pool: `.10` upward in the hotspot /24 for the maximum number of devices plus spare; gateway, DNS and server id are `.1` (this computer); lease time one hour
(renew at half). Replies to an address that does not yet exist go to the broadcast address on the hotspot interface, to the client's hardware address. Only
authenticated stations are served. The lease table is in memory (the hotspot is short-lived) and shown in the clients list with the host name (cleaned).

### 4. DNS forwarder

`dns::Forwarder`, UDP 53 bound to the hotspot address. Forwards the query to the uplink's resolvers (read from the machine's own resolver configuration at start and
when the uplink changes), under a fresh random id, and restores the client's id on the answer. No cache at first (less to get wrong; cache later). Answers the name
`iyakki.lan` (the gateway) with the hotspot address. UDP only: a client that needs TCP gets a truncated answer from the resolver and falls back to its next resolver or
to retry (later: TCP forwarding). Refuses queries from outside the hotspot subnet, over 512 bytes, with more than one question, opcodes other than QUERY (not implemented
reply), and when more than the allowed number of queries are pending.

### 5. Subnet choice

`subnet::choose_subnet(avoid)` returns a /24 that overlaps nothing on the machine: every address and route on every link (the uplink, VPN, k3s and container ranges, direct
links). It tries `10.42.0.0/24`, `192.168.43.0/24`, `192.168.137.0/24`, `10.43.0.0/24`, `172.20.10.0/24`, then `10.64.0.0` upward. When the uplink changes to a network
that overlaps the hotspot's, the manager restarts the hotspot on a new subnet (clients renew). The machine takes `.1`.

### 6. NAT and firewall through the existing writer (no shell)

The firewall (`docs/FIREWALL.md`, `crates/iyakki-fw`) has an input chain only and writes nf_tables through its own netlink code. The hotspot adds, through the same writer
(`nl::tablegen`, new "hotspot zone", in the hardware step, not in this crate):

* **Input**, hotspot interface: accept UDP 67 from `0.0.0.0` to 255.255.255.255 or the hotspot address, accept UDP 53 to the hotspot address; drop everything else (EAPOL
  is below the IP layer and needs no rule). Placed after the base rules and before ping, presets and user rules, exactly like the direct-link zone.
* **Forward** (a new chain, policy drop): hotspot interface → uplink interface accept (connection tracking for replies); established/related from uplink → hotspot accept;
  hotspot → hotspot drop when isolation is on; hotspot → private ranges of the uplink LAN drop unless allowed.
* **Postrouting** (nat): masquerade for the hotspot subnet out of the uplink interface.
* `net.ipv4.ip_forward=1` only while the hotspot runs; the previous value is restored when it stops (the existing `fwextra_net::share_commands` / `unshare_commands`
  already do this for wired sharing; the hotspot will use the same validators, `valid_iface`, but the rules themselves go through the writer, not through shell strings).
* Interface names are validated before they enter a rule. Rules are removed on stop, on idle time-out, and at boot (stale rules from a crash are removed by name).
* The uplink is "any link that has a default route", chosen in Settings; when it loses its route the page shows "no internet to share" and the clients keep their
  Wi-Fi link (their internet fails clearly rather than being switched off silently).

## Hardware requirements

* The card must report `NL80211_IFTYPE_AP` in `NL80211_ATTR_SUPPORTED_IFTYPES`. Most Intel, Atheros/Qualcomm, Mediatek, Ralink and Realtek (mac80211) cards do; many USB
  dongles do; Broadcom `brcmfmac` does with firmware support; cards whose firmware blocks AP mode (some Intel on 5 GHz without "IR" permission) do not.
* Firmware for the card must be on the image (the firmware-by-button rule of the release plan applies). **The Mac mini's Wi-Fi has no firmware on the image and cannot be used.**
* The uplink must be another link. If it is Wi-Fi, a second card is needed in the first version.
* 2.4 GHz channel 1/6/11 first. 5 GHz needs the regulatory domain and "initiate radiation" permission; a later step.
* Kernel options (`CONFIG_CFG80211`, `CONFIG_MAC80211`, `CONFIG_NF_NAT`, `CONFIG_NF_TABLES`, masquerade) are expected to be present because the firewall and client mode already need them;
  the hardware step checks `CONFIG_MAC80211` AP support and the driver module list.

## Settings page: "Share internet" under Network

Under Settings, Network (`crates/iyakki-shell/src/apps/`, next to the Wi-Fi page; the page is written in the hardware step, not now):

```
 Share internet                                           [ Off ]  (switch)

 Share through     [ Wi-Fi card: wlan1 (Intel AX200) ▾ ]       only cards that can be an access point are listed
 Internet comes from [ Ethernet: eth0 ▾ ]                      any link that has a default route
 Network name      [ IYAKKI-1A2B ]
 Passphrase        K7MQ-2XWD-9PRT-HC4N   [ New ]  [ Show / hide ]   (QR code later)
 Devices at most   [ 8 ]      Switch off after [ 2 ] hours without a device
 [ ] Let devices on the hotspot see each other          (off)
 [ ] Let devices reach my local network                 (off, administrators)

 Connected devices                                        Name        Address        Time
                                                          pixel-7     10.42.0.10     12 min    [ Remove ]
 Status line: "Sharing eth0 through wlan1 as IYAKKI-1A2B. 1 device. Switches off in 1 h 48 min."

 ⚠ Everyone who knows the passphrase can use this internet connection and everything you
   download through it counts as yours. Turn it off when you do not need it.
```

* The first time, a confirmation dialog repeats the warning. Start is refused with a plain message when: no card can be an access point, no link has internet, the card is
  in use (offer to stop the client side), the passphrase or name is invalid, or the platform says "not supported on this build".
* The passphrase is shown (this is the one place where it is shown), can be regenerated (disconnects everyone), and is stored in the vault.
* A badge in the shell status area shows while sharing, with the number of devices.

## Test plan

Hardware-free (written now, `cargo test -p iyakki-hotspot`):

* AES with the FIPS-197 vectors (128, 192, 256); RFC 3394 key wrap vectors; PBKDF2-HMAC-SHA1 with RFC 6070; the IEEE 802.11i annex H passphrase-to-PMK vectors; the PRF
  and PTK against an independent implementation (Python `hmac`), HMAC-SHA1 against RFC 2202 (in `iyakki-sec`).
* EAPOL-Key encode/decode/MIC with golden flag words, fuzz-style random bytes.
* Handshake: full run against a fake supplicant written separately from the standard (raw byte offsets, its own MIC check); wrong passphrase; lost messages and retransmission
  with new replay counters; stale and forged replies; downgrade (RSN element changed); client choosing TKIP; replayed message 3; timeout; junk.
* DHCP: the whole conversation, NAK cases, INFORM, DECLINE, RELEASE, expiry, pool exhaustion and no starvation, strangers ignored, hostnames cleaned, random mutation of packets.
* DNS: forward and restore id, answer matching by id and question, subnet check, oversize, odd opcodes, compression pointers, bounded table, own name, junk.
* Subnet chooser against the uplink, a VPN and container ranges; settings validation and generation.
* nl80211: golden bytes for the family request, set interface, stop AP, delete station, station authorize; structure checks for START_AP, keys, new station, frames; parsers for
  the wiphy dump, events and acknowledgements; the control layer against a fake transport (order of calls, sequence numbers, kernel refusal); junk.

Needs a real machine and card (cannot be proved by these tests):

1. Does the card list `AP` and can it start one (driver, firmware, regulatory domain)? Do phones see the name? (beacons and probe responses are accepted by real radios.)
2. Do real phones' supplicants accept our M1/M3 (key descriptor flags, RSN element, GTK KDE padding, replay counters)? Several vendors are picky; test iPhone, Android, Windows, macOS, Linux (wpa_supplicant), and a printer or IoT device.
3. Do the kernel and driver accept the order of `NEW_STATION`, `NEW_KEY` and `SET_STATION`, and does data flow after authorization (CCMP in hardware)?
4. EAPOL through the control port versus `AF_PACKET`, per driver.
5. Real DHCP clients (Android and iOS are strict about options and broadcast flags), real DNS resolvers (EDNS sizes, DNSSEC answers over 512 bytes), IPv6-only behaviour.
6. NAT end to end through the real nf_tables writer; the firewall rules in a live kernel; the forwarding switch restored on stop.
7. Throughput, many clients, power save (PS-poll and TIM handling, handled by mac80211 once stations are added), roaming of clients away and back, long runs (group key time-out is not used).
8. The Settings page on a screen and the stop-after-idle timer in real time.

Test machines: the **Mac mini cannot be used** (its Wi-Fi has no firmware on the image). The development PC has an Intel `wlp3s0`, but it is that PC's **only internet link**,
so it must not be touched by any test. Real hardware tests need a second, separate card (for example a USB Wi-Fi dongle with a mac80211 driver) on a machine whose internet is
on Ethernet, and a VM with USB pass-through of that dongle is the safest first step.

## Roadmap of the real access point (owner request 2026-11-04: "plan for wifi hotspot too")

Order of work: the wired/USB sharing manager comes first (section "Wired, USB and Thunderbolt sharing"); the Wi-Fi access point plugs into it as one more
downstream type. Steps 3 to 5 of the old build order (manager, firewall zone, Settings page) are shared with it and are not repeated here.

### P1: real nl80211 transport and AP bring-up on VIRTUAL radios (built, no hardware)

* `crates/iyakki-agent/src/nl80211_os.rs`: generic netlink socket implementing `NetlinkTransport` (family lookup with multicast groups, sequence numbers, time-outs,
  extended-ack text, events kept in a queue, `config` and `mlme` groups joined), `AF_PACKET` EAPOL channel behind the `EapolChannel` trait, `poll`, a UDP socket
  bound to one interface, and the capability `probe`. Minimal `unsafe`, one SAFETY comment each; `iyakki-hotspot` stays `#![forbid(unsafe_code)]`.
* `crates/iyakki-agent/src/hotspot_radio.rs`: the event loop (probe, AP mode, register frames, START_AP, group key, auth/assoc answers, handshake, keys, authorize, DHCP).
* `crates/iyakki-hotspot/src/caps.rs`: the pure capability probe and `can_host_ap` (see "Generic principle").
* Test bed `crates/iyakki-hotspot/tools/hwsim-test.sh`: a QEMU microVM (KVM, no network device, 1 GB, hard time-out, files only under `~/iyakki-work/hwsim`), a small
  kernel with `cfg80211`, `mac80211` and `mac80211_hwsim` built in (`hwsim-kernel.frag`, `hwsim-kbuild.sh`; the distribution kernel in `/boot` is root-only, so
  it cannot be used without sudo), an initramfs with static busybox, the host's `wpa_supplicant` and `iw` with their libraries and our test binary. Two boots:
  right passphrase (association, 4-way handshake by OUR authenticator, keys, COMPLETED, DHCP from OUR server, ping across the virtual air in a second network
  namespace) and wrong passphrase (must fail at the handshake). Acceptance: every line prints PASS.
* Findings of P1 are recorded in the next subsection. P1 proves the netlink call sequence and the handshake against the REAL mac80211 and the REAL wpa_supplicant;
  it cannot prove radio behaviour, firmware, regulatory limits or phone quirks (that is P2).

### What the virtual radios showed (P1 findings, 2026-11-04)

Run: `crates/iyakki-hotspot/tools/hwsim-test.sh` (logs `~/iyakki-work/hwsim/hwsim-good.log` and `hwsim-bad.log`). Result: all 15 checks PASS: the real `wpa_supplicant`
associates to our access point, completes the 4-way handshake against OUR authenticator, reaches COMPLETED, gets `10.42.0.10` from OUR DHCP server and a ping to `10.42.0.1`
crosses the virtual air with CCMP; with a wrong passphrase the supplicant reports `4-Way Handshake failed - pre-shared key may be incorrect` and our side logs `wrong passphrase`, no key
is installed and no station is authorized. What this taught us (and what the code does about it):

1. **Authentication and association are NOT done by the kernel for a mac80211 access point.** (The task brief assumed they were.) Frames are delivered to userspace only after
   `REGISTER_FRAME` for authentication (0x00b0) and association request (0x0000), and we must answer them with `NL80211_CMD_FRAME` and add the station with `NEW_STATION`
   (authenticated + associated flags) ourselves, as hostapd does. So the `mgmt` module is fully needed. The kernel does build and send the beacons (head + tail, it adds the TIM) and
   answers probe requests from the template.
2. **EAPOL over `AF_PACKET`** on the AP interface works for both directions even for a not yet authorized station (the control-port route was not needed for P1; keep it as the
   fallback the plan already names).
3. **Optional attributes can make `START_AP` fail**: `NL80211_ATTR_INACTIVITY_TIMEOUT` makes a radio without `NL80211_FEATURE_INACTIVITY_TIMER` (hwsim) answer EOPNOTSUPP. The builder now leaves it
   out when 0, and the runner sets it from the probe's feature flags. Principle for the future: every optional attribute is gated by the probe, never by a chip name.
4. **Interface must be down for `SET_INTERFACE` and up before `START_AP`**; the group key goes in after `START_AP`; the pairwise key after message 4; `SET_STATION` authorized last.
5. **The `GET_REG` reply has no terminating message** unless the acknowledgement flag is set; the transport ends a request at ACK, DONE, or the single controller reply, so non-dump `GET_*` requests carry the ACK flag.
6. **A split dump (`NL80211_ATTR_SPLIT_WIPHY_DUMP`) is required** to get bands, combinations and ciphers; the old dump is truncated. Parts of one radio arrive in several messages and are merged.
7. **Real kernel check of the probe parser** (read-only, on the development PC, nothing changed): the Intel card reports driver `iwlwifi`, a self-managed regulatory domain with country `00`, 14 channels in 2.4 GHz (13 usable
   for an AP), 5 GHz with 9 usable, 6 GHz none; two interface combinations (managed + AP + P2P-device, one of them on a single channel). The decision says "yes, channel 6" and "with client: the AP follows the client's channel".
8. **The distribution kernel in `/boot` is root-only** on a normal Ubuntu install, so the test uses its own small kernel (about 2 MB, built once; `hwsim-kernel.frag`, `hwsim-kbuild.sh`) with hwsim built in.
   Nothing is loaded on the development PC and the VM has no network device.

What virtual radios cannot show: real RF (beacon timing, power save with a TIM, retries, roaming), firmware and regulatory behaviour, driver-specific refusals, and the pickiness of phone supplicants (P2).

### P2: hardware interoperability on a real USB dongle (acceptance test only, never a dependency)

The product is generic (no chip lists, no driver code). For the *acceptance run* we still need one real card whose driver has AP support and open firmware, on a machine
whose internet is NOT on that card (the development PC's Intel card is its only link and is never touched):

| Adapter | Driver / firmware | How sure |
|---|---|---|
| Atheros AR9271 (for example TP-Link TL-WN722N **v1**, Alfa AWUS036NHA) | `ath9k_htc` in the kernel; firmware `htc_9271.fw` is open and in `linux-firmware`; 2.4 GHz only; AP mode is the textbook case | Sure about AP support and open firmware. Not sure which retail dongle revisions still carry the AR9271 (v2 and v3 of the TL-WN722N do not): check the USB id (`0cf3:9271`) before buying. |
| MediaTek MT7610U / MT7612U (for example Alfa AWUS036ACM = MT7612U, several TP-Link/Edimax AC dongles) | `mt76x0u` / `mt76x2u` in the kernel; firmware blobs from `linux-firmware` (redistributable, not open source); 5 GHz capable | Sure that the drivers support AP mode. Not sure about the exact interface combination table, DFS behaviour, or which retail models still use these chips. |

Not recommended for the first acceptance run: Realtek out-of-tree drivers, Broadcom `brcmfmac` (FullMAC, firmware is not redistributable on the Mac mini), anything needing DKMS.
**The image's firmware pinning must carry the blob** (`htc_9271.fw`; `mt7610u.bin` / `mt7662u.bin` and `mt7662u_rom_patch.bin`): the dongle must work from the image on
the machine under test, otherwise the test says nothing about the product. Same run order as the VM test (right passphrase, wrong passphrase), then: iPhone, Android,
Windows, macOS, Linux phones/laptops, a printer or IoT device; the capability probe is run first and its verdict is compared with what the card really does.
Acceptance: every device joins, gets an address, loads a page through the NAT; the wrong passphrase is refused; 8 devices at once; one hour without a drop; stop and start 20 times.

### P3: integration into the sharing manager, Settings page, country

* A downstream type "Wi-Fi hotspot" in the manager (start, stop, idle switch-off, audit lines, clean-up at boot); it calls `can_host_ap` to list the cards that can host and to
  show the one-line reason for the others; "use another uplink or another card" when the same radio is the uplink and the combination table forbids it.
* Settings, Network, Share internet: the entry, card choice, name, passphrase (vault), devices, band (2.4 GHz first).
* **Regulatory domain**: the country code is machine data (Settings, region), set through `NL80211_CMD_REQ_SET_REG` (`nl80211_os::set_country`). A radio with a self-managed
  regulatory domain (firmware decides) ignores it: the page says so. Without a country (`00`) only channels the kernel already allows are offered. 2.4 GHz 1, 6, 11 first;
  channels 12 and 13 are never chosen; 5 GHz only channels without radar detection (36 to 48, 149 to 165) and only when the probe says they are usable; DFS later.
* Link and address set-up move from the `ip` program (test bed) to rtnetlink; one `poll` loop with the manager's timers; the NAT and firewall zone through `iyakki-fw`.

### P4: WPA3-SAE, 5 GHz DFS, FullMAC paths

* WPA3-SAE (needs P-256 and hash-to-curve in our own crypto; the kernel must support SAE for the AP: `NL80211_FEATURE_SAE`), 802.11w, transition mode (WPA2/WPA3).
* 5 GHz DFS with radar detection only where the driver offers it (`NL80211_EXT_FEATURE_DFS_OFFLOAD` or mac80211 DFS), 40/80 MHz, HT/VHT rates, one-card concurrent mode.
* `ApMode::PskOffload` (card does the 4-way handshake; pass the PSK in `NL80211_ATTR_PMK`) and `ApMode::CardManaged` for FullMAC; both are already *detected* by the probe and refused with a sentence.

### Risks

| Risk | Mitigation |
|---|---|
| Phones are picky about EAPOL flags, RSN element, GTK KDE, replay counters | P1 uses the real wpa_supplicant (strict on the same points); P2 tries the vendor list; the handshake code is covered by the fake supplicant |
| Driver orders of calls differ (NEW_STATION before or after the assoc response, key install timing) | Order copied from the kernel's documented flow and proved on mac80211; the log shows the failing call and the kernel's extended-ack text; P2 finds the rest |
| The kernel refuses attributes some radios do not support (found: INACTIVITY_TIMEOUT) | Optional attributes are sent only when the probe's capability says so |
| Regulatory: no channel allowed, or firmware overrules the country | Probe reports it before start; page explains; country set through nl80211 |
| Radio shared with the uplink | Combination table decides; clear refusal sentence |
| Firmware not on the image | Firmware pinning carries the blob; page says "device present, firmware missing" |
| Performance with legacy rates only | Accepted for first version; HT/VHT in P4 |
| Our crypto code is not constant-time AES | Used once per client on handshake data; documented |

### Acceptance tests

* P1 (automatic, no hardware): `crates/iyakki-hotspot/tools/hwsim-test.sh` prints PASS for each check; Rust: `cargo test -p iyakki-hotspot caps`, the framing tests of
  `iyakki-agent::nl80211_os::tests`, and the `#[ignore]` tests that need the VM or a real Wi-Fi radio.
* P2: the device list above on a real dongle, with the capability probe's verdict recorded beside the result.
* P3: manager integration tests with fakes; the Settings page on a screen; reboot clean-up; country change.
* P4: SAE client test with a WPA3-capable phone and `wpa_supplicant` on Linux; DFS only on a card whose driver reports radar detection.

## Open questions for the owner

* Default idle switch-off 2 hours: right? (the field allows 1 to 72)
* Should "Let devices reach my local network" exist at all in the first version (it is off and admin-only), or stay out until asked for?
* Is the name `iyakki.lan` for the gateway acceptable, or should it be `iyakki.local` (clashes with mDNS)? `.lan` was chosen to avoid mDNS.


## Wired, USB and Thunderbolt sharing (built first, owner order of 2026-11-04)

* `iyakki share <start|stop|status|list|policy>` and Settings, Network, Share internet (key `i`). The service runs inside the system's first process
  (`sharing::real::start_system`), polls `<data>/share/request.json`, writes `status.json`. Off after every boot; one session at a time.
* Uplink: any link with a default route, chosen by the person, never the shared link. Downstream: an Ethernet, USB (gadget, RNDIS/ECM/NCM adapter) or Thunderbolt link
  with no address of its own. Private /24 from the subnet chooser (avoids every address on the machine); the machine is `.1`.
* DHCP on UDP 67 (bound to the link with `SO_BINDTODEVICE` before `bind`) and DNS on UDP 53 at the gateway address, same binding; the resolver socket follows the routes.
* Firewall: table `inet iyakki_share` written by `iyakki-fw`'s typed writer (hooks input/forward/postrouting, `ct state`, masquerade were added to `nl::tablegen`);
  input accepts only DHCP and DNS from the link; forward: replies back, link to link dropped (isolation), the uplink's private ranges dropped unless allowed, the link to
  the uplink accepted, everything else to or from the link dropped (policy accept so containers keep forwarding); postrouting masquerade. The Firewall's own table gets a
  sharing zone (`iyakki_fw::service::set_share_zone`) because its base policy would otherwise drop DHCP. `net.ipv4.ip_forward` is restored on stop; a record is
  written before the first change so the next start removes what a crash left (table, zone, address, forwarding). `iyakki usb share-internet --apply` uses the same writer now.
* Idle switch-off (default 2 h, 1 to 72) from the lease table and the link's byte counters; uplink or cable lost: everything is torn down with one plain line.
* Policy (`sharing::policy`, `<data>/share/policy.json`, administrators change it, audited with before and after): who may start per kind (wired and USB, Wi-Fi, Bluetooth:
  admins, users or nobody; default admins), device limit, hard maximum session length, allowed shared links and uplinks. Enforced in the CLI, the page and the service; a
  running session stops within about two seconds when the policy no longer allows it (audit line; no notification hub call yet). Tenant-config entry: skipped (see report).

Only verifiable on a machine with a real cable: see the test steps in the final report of this work (Mac mini, Ethernet to a laptop).

## Bluetooth (later work; modelled now)

Owner request of 2026-11-04. The sharing code has the kinds `BluetoothNap` (this machine is a Network Access Point for phones) and `BluetoothPanu` (this machine uses a paired
phone's tethering); their `probe()` says "not available on this build", the page lists both with that reason, and a plan can already describe a downstream that is a bridge of
several member ports under one address (`Plan::members`). What the later work needs:

* NAP downstream: kernel `CONFIG_BT_BNEP=m` (+ `BT_BNEP_MC_FILTER`, `BT_BNEP_PROTO_FILTER`) and bridge support (`CONFIG_BRIDGE`); the Mac kernel fragment lacks them today.
  A raw L2CAP socket on PSM 15 (BNEP), the BNEP setup handshake in our own code, `BNEPCONNADD` ioctls to create `bnepN`, each added to one bridge `iyakkipan0` that carries
  the /24, DHCP, DNS and NAT like any other downstream; an in-house minimal SDP server and record for the NAP service (UUID 0x1116); pairing through the existing btmgmt client.
  Isolation: bridge port isolation, or the typed nftables `bridge` family if the writer is extended to it (not today: `tablegen` writes `inet` only); otherwise documented as not available.
* PANU uplink: connect to a paired phone's NAP (UUID 0x1115), get an address by DHCP on `bnepN`; the manager treats it like any link with a default route (higher metric than wired).
* Policy kind `bluetooth` already exists. Tests with the same fakes (a fake L2CAP socket and fake ioctls), real proof needs a phone.
