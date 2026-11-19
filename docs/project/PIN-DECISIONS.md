# Pins for the downloads inventory: what is pinned, what floats, what needs the owner (F-QA-06)

Generated from `legal/downloads.txt` on 2026-11-14 (Q-C) by reading the pin kind column; the notes are by hand. Nothing here was downloaded or verified against a vendor tonight: a digest or hash is written into the inventory only when the code already carries it or a person has read it from the vendor.

| Pin kind | Rows | Count |
|---|---|---|
| digest | graph-db, clickhouse, gitea, prometheus, elasticsearch, opensearch, matrix, registry, lemonade, gpu-nvidia, fw-hardware | 11 |
| version | seaweedfs, rustfs, gpu-vulkan, cli:espflash, cli:7zip | 5 |
| checksum | nodejs, k3s, claude-code, github-copilot, ollama, ffmpeg, linux-apps, cli:kubectl, cli:k3s, cli:helm, cli:terraform, cli:opentofu, cli:gh, cli:arduino-cli, cli:nodejs, cli:uv, cli:claude-code, cli:github-copilot, cli:ollama, cli:ffmpeg, cli:grpcurl | 21 |
| none-vendor | python, duckdb, cli:aws-cli, cli:gcloud, cli:esptool, cli:azure-cli, cli:python, cli:codex, cli:gemini-cli, cli:aider, cli:duckdb, cli:openvpn, cli:cryptsetup, cli:dislocker, cli:exfatprogs, cli:ntfs-3g | 16 |
| floating | mac-firmware, mac-camera, linux-windows | 3 |
| n/a | kokoro, voice-pack, mac-host, ffmpeg-libs, cli:ssh | 5 |

## What can be pinned without a decision, and was not, and why

- Nothing was pinned in this round. Every candidate below needs either a vendor download that was not made (no digest may be invented) or a product decision. The code and the inventory are unchanged by this page.

## Rows that need an owner decision

1. **checksum rows (the version floats, the vendor checksum is verified at download):** kubectl, k3s, helm, terraform, opentofu, gh, arduino-cli, nodejs (LTS line), uv, claude-code, github-copilot, ollama, grpcurl, ffmpeg (MD5 only), linux-apps. The checksum is the vendor's own integrity check, so the download is verified, but the tool is whatever the vendor calls latest that day. Options: (a) keep as is and say so on the screen (today); (b) pin a version and a SHA-256 per architecture in the code and move the update to a release step (safer, a person must refresh the table); (c) both: the person may choose latest or the pinned one. Needs the owner: it changes what the Install button does.
2. **none-vendor rows installed with pip or npm** (duckdb, esptool, azure-cli, aider, codex, gemini-cli): the version is pinned; the hashes are on PyPI and npm but dependencies float. A real pin is a lock file with hashes (`pip install --require-hashes -r`) per tool, built on a machine with the network and kept in the repository. Not done: it is a build step of its own (one lock per tool and architecture) and each lock must be re-made at each version change.
3. **none-vendor rows whose vendor publishes only a signature** (aws-cli: PGP .sig; the Amazon key is not yet in `legal/aws-cli-pgp.asc`): pin the key once a person has read its fingerprint from Amazon's page; the in-house OpenPGP check is ready. gcloud publishes no checksum: pinned by version only, the screen warns.
4. **none-vendor rows with the address and hash from a setting** (openvpn, cryptsetup, dislocker, exfatprogs, ntfs-3g): the person supplies the address and the SHA-256 (setting `vpn.openvpn_download` and its siblings); there is nothing to pin in the code by design. Decision: keep (the person is the pin), or ship an official list of known-good addresses with hashes (a store catalogue entry, signed).
5. **floating rows:** mac-firmware and mac-camera (Apple files for the person's own Mac: Apple publishes no stable hash list; the extraction table holds hashes per file after the first download) and linux-windows (apt inside the sandbox, versions not pinned: a Debian snapshot date would pin them). Decision: pin by Debian snapshot date for linux-windows, and leave the two Apple rows as floating with the warning, or remove them.
6. **python (3.12.16, python-build-standalone 20261026):** uv checks the build against its own SHA-256 table; the project's SHA256SUMS could be fetched and kept as a second check. Small and safe once a person fetches the file; not done tonight.
7. **seaweedfs and rustfs (image tags without a digest):** the first real install records the imageID; until a person reads the digest from the registry they stay by tag only (rustfs is an alpha release: owner may prefer to remove it).

## What the gate already enforces

The gate (`licence_gate`) fails when an item has no row, a pin in the file differs from the code, a tool without a vendor checksum is described as verified, or a store entry has no licence. It does not fetch anything, so a row that says digest is only as good as the person who copied it.
