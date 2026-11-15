# Open Impl in rows M to S (Q-C, 2026-11-14)

One line per partial row with Impl from 60 to 99 in the areas MEDIA, MEM, MSG, NAME, NET, PDF, PICT, PKG, PWR, QA, RADIO, RAG, REL, SBX, SCAN, SCRN, SDK, SETUP, SHARE, SHELL, SND, SSH.
Format: row, Impl/Test, what is missing, what blocks it (H hardware, V vendor or live service, O owner decision, I an image or cluster nobody has built), the slice that needs none of these, and its state.

| Row | Impl/Test | Missing | Blocker | Software-only slice | State |
|---|---|---|---|---|---|
| F-SHELL-40 | 98/50 | a session that survives a dead process; a look at the crash card on a device | O (no auto sign-in), H | none | nothing to do |
| F-MEDIA-02 | 97/70 | a real player (TV, VLC, phone app), multicast group, announcements, port 1900 shared | H, V | none (the sockets are tried on loopback) | nothing to do |
| F-NET-07 | 95/50 | a real access point and adapter | H | none (planner, enterprise and WEP entries, captive-portal probe are all tested) | nothing to do |
| F-NET-18 | 95/50 | a real keyboard, mouse or game pad; reconnect after a reboot | H | none | nothing to do |
| F-MEDIA-05 | 95/70 | the optional yt-dlp extractor for video sites | V (third-party program, installer only by a click), O | a Setup item that downloads yt-dlp on the person's click and a media_download path that uses it when it is installed | open, large |
| F-QA-05 | 95/70 | images that pass the gate (owner decisions on firmware and legal-info rows); own zst decoder; whole-disk partition tables | O, I | the partition-table walk of a whole-disk image (a reader of MBR and GPT in `fsimage.rs`) | open, small |
| F-SETUP-18 | 95/50 | independent checks of the firmware packs, camera firmware and the Mac firmware against a real device | H, I | none left in code | nothing to do |
| F-SETUP-06 | 95/50 | the other lists of the shell on the shared scrolling helper (KI-641); the touch screen and pad | H | convert more lists to `scrolllist` | open, many small |
| F-PICT-01 | 95/50 | SVG clip, mask, filter and pattern parts (KI-714); script `devicePixelRatio` tied to the density | none | SVG parts, devicePixelRatio | open |
| F-SHELL-13 | 90/50 | multi-threaded software drawing; a measurement on the Mac mini | H for the measurement | the threaded drawing | open, large and risky |
| F-NET-27 | 90/50 | a real phone or headset | H | none | done in code 2026-11-14 |
| F-SBX-03 | 85/50 | the namespace tier inside a pod | I (needs a pod to design against) | none | nothing to do |
| F-RAG-14 | 85/50 | the reranking request tried against a real Lemonade server (KI-744) | V | none | nothing to do |
| F-RAG-16 | 85/50 | no browser button; no refresh of a changed page by itself; no crawling | none | a browser key "Keep in documents" that keeps the page the person is reading | open, medium |
| F-PDF-08 | 85/50 | CFF fonts and Type 1 programs with a private glyph order; no real-world PDF corpus | none for the format part | CFF `Encoding` and `charset` reading (a Type 1C reader) | open, medium |
| F-RAG-15 | 80/50 | lower-case names, scripts without capitals, a model-based extraction (KI-745, KI-980) | V for the model | none cheap | nothing to do now |
| F-NET-24 | 80/50 | a real phone's sequence and timing, latency, a second phone | H | none | nothing to do |
| F-QA-11 | 75/50 | download and checksum addresses from the signed manifest; fixed digests and cluster images; a first published manifest | O (the key), V | step 2 second half and step 4 of the backlog in DOWNLOADS-MANIFEST.md | open, security-sensitive |
| F-QA-06 | 70/50 | pins for the items still floating; the 38 REVIEW rows of the note are 20 checksum, 16 none-vendor and 3 floating today | O (each row is pinned, marked unverified or removed) | refresh the note and test the counts | slice done 2026-11-14 (note) |
| F-SHELL-62 | 70/50 | the colour mapping of apps that draw outside the palette cannot be set by a file (KI-749); not seen on a screen | H for the look | none | nothing to do |
| F-PKG-05 | 70/50 | the budgets are not measured from real images | I, H | none (script exists) | nothing to do |
| F-NET-14 | 65/50 | the IPv6 beacon socket and neighbour discovery; the ioctls, raw ARP and UDP never ran on two real machines | H | the IPv6 beacon socket (Linux only, cannot be run on a Mac) | open, not testable here |
| F-SDK-15 | 60/50 | `iyakki.k3s` namespace per app (needs the service package kind, KI-724) | I, O | none | nothing to do |
| F-REL-09 | 60/50 | no release image exists; GPU guard untested on hardware | I, H | none | nothing to do |

Rows with Impl 100 and a Test below 100 are not listed here; their open part is verification, not code.
