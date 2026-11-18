# IYAKKI quickstart

> **Two guides in one file.** The first part, *First hour on a stick or in a virtual machine*, is for someone who downloaded a release and has never seen IYAKKI. The second part (from "a 15-minute tour" below) is a tour of the tiles; its steps and anchors are unchanged.

# First hour on a stick or in a virtual machine

**For:** a person who wants to try the alpha from a release download, without building anything.
**You will get:** IYAKKI booted from a USB stick (or in QEMU), your administrator account, a model connected, a first question answered, and the knowledge of how to update, go back and report a problem.

> **Alpha preview for developers and testers, not for production.** The writer erases the drive you choose. Use a spare stick or SSD and a spare computer. What was tried on which machine: [`TESTED-HARDWARE.md`](TESTED-HARDWARE.md). Each step below says whether it was checked, and where (a "Checked" line); what was not checked is marked **Not checked**.

## 0. What you need

* A 64-bit PC (UEFI or legacy BIOS) or an Intel Mac, with at least 2 GiB of memory, and a USB stick or SSD of 8 GB or more that you can erase. Apple Silicon Macs cannot boot it on bare metal ([`HARDWARE.md`](HARDWARE.md)).
* A wired network connection is the safe choice: Wi-Fi and Bluetooth need vendor firmware that IYAKKI does not ship, and some cards have no driver at all (see [`TESTED-HARDWARE.md`](TESTED-HARDWARE.md)).
* A model to talk to (step 5). IYAKKI ships none.
* Or, instead of the stick, a Linux PC with QEMU for a virtual machine (step 2b).

## 1. Download and check the files

From the release page of the version you want, download the install image and `SHA256SUMS` (and its signature file, listed on the release page). The file names are on the release page; [`INSTALL.md`](INSTALL.md#choose-a-way) describes what each image is (`iyakki-install-x86_64.img` is the stick image).

```
sha256sum -c SHA256SUMS --ignore-missing      # Linux; on a Mac: shasum -a 256 -c SHA256SUMS --ignore-missing
iyakki release verify DIR --pubkey HEX        # needs the iyakki program (built from source or inside a running IYAKKI); DIR holds the files, HEX is the public key printed on the release page
```

**Expected:** `OK` next to each file, and a good signature from `release verify`. If either fails, do not write the file: download it again.

**Not checked:** `iyakki release verify` on a downloaded public release (there was no public release when this was written); the checksum command is the ordinary system tool.

## 2a. Write the stick

Follow [`INSTALL.md`, section 2](INSTALL.md#2-usb-stick) (the `iyakki-usb` writer: it lists only USB drives, refuses the system disk, makes you confirm the drive, and reads the stick back to check it) or, to put IYAKKI next to another system on a computer's internal disk, [`INSTALLER.md`](INSTALLER.md). Those pages are the reference; this guide does not repeat them. On Linux without the writer built: `sudo dd if=IMAGE of=/dev/sdX bs=4M conv=fsync status=progress` after checking the device with `lsblk`; a wrong device means lost data.

**Not checked:** writing a real stick from a downloaded image (the images used for this guide were disk images in QEMU; the stick path was tried by the owner on real computers, see [`TESTED-HARDWARE.md`](TESTED-HARDWARE.md)).

## 2b. Or: a virtual machine

On Linux with QEMU and the UEFI firmware package (`ovmf`), boot the image as a USB disk:

```
qemu-system-x86_64 -accel kvm -m 3072 -smp 2 -machine q35 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive if=pflash,format=raw,file=my-vars.fd \
  -drive file=iyakki-disk.img,format=raw,if=none,id=u0 -device qemu-xhci -device usb-storage,drive=u0,bootindex=1 \
  -device virtio-vga -device usb-kbd -device usb-tablet \
  -netdev user,id=n0 -device virtio-net-pci,netdev=n0
```

`my-vars.fd` is a copy of `/usr/share/OVMF/OVMF_VARS_4M.fd`; `iyakki-disk.img` is a copy of the image (QEMU writes to it, so keep the original). Click the window to capture the mouse, **Ctrl+Alt+G** releases it. More VM recipes (`./run.sh vm-disk`, UTM on a Mac): [`INSTALL.md`, section 4](INSTALL.md#4-virtual-machine) and [`LOCAL-TEST.md`](LOCAL-TEST.md). QEMU's NAT gives the guest internet, so a cloud model works in the VM.

**Checked** (2026-11-18, QEMU on a Linux PC, KVM, UEFI with the firmware above, a lean image of the alpha.6 line made on 2026-11-16, a blank data volume): the machine reaches the sign-in-or-setup screen about 40 seconds after QEMU starts, including the firmware; the screen says "Welcome, create the administrator account"; the guest gets a network address and the top bar says "Ethernet connected".

## 3. Boot the stick (Secure Boot)

Open the firmware's one-time boot menu (often F12, F9, F10, F8 or Esc; Option on an Intel Mac) and pick the stick. If the computer has Secure Boot on, you have two honest options, because the images are signed with **the project's own key**, not a key your firmware already trusts:

1. **Switch Secure Boot off** in the firmware settings for the alpha. Simplest; nothing else to do. (Recommended for a first try.)
2. **Enrol the project's key once** (Machine Owner Key, MOK), then leave Secure Boot on:
   1. Start from the stick. A grey box "Verification failed: (0x1A) Security Violation" appears. Press Enter.
   2. When "Press any key to perform MOK management" shows, press a key within 10 seconds (the blue MokManager screen).
   3. Choose **Enroll key from disk**, then the stick, then `EFI`, `IYAKKI`, `iyakki-mok.cer`. **View key 0** shows the fingerprint: compare it with the one in `EFI/IYAKKI/README-SECURE-BOOT.txt` on the stick (and with the one on the release page).
   4. **Continue**, **Yes**, **Reboot**. No password is asked for a key chosen from a disk. IYAKKI starts. This happens once per computer.

Details, and why the chain is built this way: [`SECURE-BOOT.md`](SECURE-BOOT.md). Apple T2 Macs also need the Mac's own security setting changed; see [`MAC-HARDWARE.md`](MAC-HARDWARE.md).

The boot menu (GRUB, or IYAKKI's own loader) offers, besides the normal entry, safe graphics, a hardware check, "Show boot messages" and an installer. If the screen stays black, try safe graphics first.

**Checked:** the stick image's boot menu in QEMU lists IYAKKI, Install to a disk, Safe graphics, Show boot messages, Hardware check and more (23 entries on the lean image). The loader chain with Secure Boot firmware and MokManager is exercised by a project QEMU test ([`BOOT-TEST.md`](BOOT-TEST.md)).
**Not checked:** the MOK steps above on a real computer during this guide; they are copied from `SECURE-BOOT.md`, and the project's own notes record that a real PC with Secure Boot on and the key enrolled (an HP laptop and a Lenovo laptop) booted, but a first-time enrolment from a public release has not been walked through by a newcomer.

## 4. First sign-in

On a blank data volume the first screen is **Welcome: create the administrator account**. Type a name (lowercase letters, digits, `-`, `_`), then a password of at least 8 characters twice (Tab moves between the fields, Enter or the **Create account** button continues). You are then taken to the sign-in screen: sign in with the same name and password.

Passwords are not recoverable by the project; the vault that holds your keys opens with your password ([`ADMIN-GUIDE.md`](ADMIN-GUIDE.md#forgotten-admin-password) for what an administrator can do).

A card in the corner may ask **Set the time zone from the network?** (a lookup at a public service) with **Use UTC** and **Not now** as the other answers. Choose what you like; Settings, Time zone and clock changes it later.

**Expected:** the launcher with tile groups (Ask & Create, Build, Media, Data, Devices & System) and your name and role in the top bar. On the launcher, the keys `o`, `q` and **Esc sign you out** (a card asks first on `o`); inside an app Esc goes back to the launcher.

**Checked** (QEMU, 2026-11-18): creating an account, signing in, and that the account is still there after the virtual machine was stopped and started again. **Not checked:** the first boot on a real stick (the data volume is formatted on the first start, which takes a few seconds on a real drive).

## 5. Your first question

IYAKKI ships no model, so the first thing CHAT says is **"No chat model selected yet. Open Settings, Models, add a provider and pick a model"**. That is expected. Open **Settings** (the gear in the top bar, or `s`), **Overview**, and press **Choose the AI model** (or **Fix it** on the "No AI model is chosen" line). Then follow [section 2 of the tour below](#2-connect-a-model): a model server on your network or on this computer (Ollama), or a cloud provider with your own key (stored in the encrypted vault, never in a settings file). Press `m` on a model to make it the chat model and `c` to test it.

Then open **CHAT**, type "What can you do on this computer?" and press Enter. Press `Ctrl+G` to switch **Agent** mode on so the assistant may use tools; every risky call pauses with **Allow once / Allow for this session / Deny**. Nothing is allowed by silence.

**Checked** (QEMU): the Overview line "No AI model is chosen" with its **Fix it** button, and the CHAT message above. **Not checked** in this guide's run: a real answer from a model (no model was connected in that run; model connections to Ollama, an OpenAI-compatible server and a Lemonade server were tried by the project, see the feature rows).

## 6. Where your files and vault live

* Your home folder is `/home/<your name>`: downloads, photos, chats and workspaces. On an installed stick or disk, `/home` lives on the data volume (label `IYAKKI-DATA`), so it survives restarts. If Settings, Storage says "RAM" you booted without a data volume and everything is lost at power-off.
* The system's own data (accounts, the audit log, settings and the encrypted vaults) is under `/var/lib/iyakki` on the same data volume. Vaults are encrypted; the rest is plain, so keep any copy private. **Settings, Where my data is kept** and **Settings, Backup and restore** show and copy it ([`BACKUP.md`](BACKUP.md), [`USER-GUIDE.md`](USER-GUIDE.md#where-your-files-live)).
* The vault holds your API keys and other secrets. It opens when you sign in with your password and closes when you sign out or lock the screen.

## 7. How to update

An update is a folder (a "pack") that holds `update.json` and a `boot/` folder, signed by the project. Put it on a USB stick or in a folder, then **Settings, System update** (administrators): the page lists the packs it finds, says in plain words what changes, and asks before it writes (**Install**); restart when you are ready. Your files, accounts and settings are not touched. Full description: [`UPDATES.md`](UPDATES.md). The older instruction "write a new image" ([`INSTALL.md`, section 7](INSTALL.md#7-upgrade-back-up-uninstall-recover)) still works and erases the drive, so back up first.

**Not checked** in this guide's run: an update from the System update page of a blank-disk VM. (The update path was exercised through developer mode on a Lenovo laptop and a Mac mini 2018 for alpha.6, see the release notes.)

## 8. How to go back

Every update keeps the version it replaced. Two ways back:

* **Settings, System update, Roll back** (asks first), then restart.
* **At boot**, choose **IYAKKI (previous version)** in the boot menu. The menu falls back to it by itself when the new version fails to start ([`BOOT-LOADER.md`](BOOT-LOADER.md)).

The "previous version" row exists only after the first update: a freshly written stick does not have one (checked: it is not in the menu of the fresh image). Your data is not touched by going back.

## 9. How to report a problem

1. Run the checks: open the **DOCTOR** tile (Devices & System) and press `r`, or in a terminal run `iyakki doctor --detail` ([`DOCTOR.md`](DOCTOR.md)). Each finding has one fix button.
2. Make a support file: **Settings, Overview, Share diagnostics** (or `s` in DOCTOR). It shows you exactly what it holds, with secrets removed, and saves a text file `iyakki-diagnostics-YYYYMMDD-HHMMSS.txt` in your home folder or on a stick. Nothing is uploaded; you attach the file yourself.
3. Open an issue on the project's GitHub repository (the bug report form asks for the version, the machine model, the diagnostics and the steps). Read the file before you attach it. Security problems go through [`../SECURITY.md`](../SECURITY.md), not a public issue.

`iyakki dev doctor` (the same report from another computer, over developer mode) exists in the tree, but [`DOCTOR.md`](DOCTOR.md) still lists it under "not built" and the live path was not tried on a device: use the first two steps.

**Checked:** the Settings overview shows the **Share diagnostics** button. **Not checked:** producing and reading the file in this guide's run.

---

# IYAKKI quickstart: a 15-minute tour

Building and installing instead? See [`QUICKSTART-BUILD.md`](QUICKSTART-BUILD.md) (build, window mode, write to disk, update, cloud).

**For:** someone who has IYAKKI running (the easiest way is window mode, see [`INSTALL.md`](INSTALL.md)).
**You will get:** a signed-in system, a model that answers, an AI IDE project deployed to a local k3s cluster, a table
and a dashboard, a story read aloud, and Claude Code installed from the CLI tile.

Each step ends with **Expected**. If it does not happen, see [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md).
Steps 5 and 7 need extra software (podman or docker, a local k3s, a speech engine); skip what you do not have.

> Alpha note: this tour was written from the source and the rendered screens. A person on a clean machine has not
> walked through it end to end yet, so report anything that differs ([`../CONTRIBUTING.md`](../CONTRIBUTING.md#reporting-issues)).

## 1. Sign in

Run `./run.sh window`. On first launch the setup screen asks for an administrator name and a password (8+ characters,
twice). Press Tab between fields and Enter to continue.

**Expected:** the launcher with a grid of tiles and chips (Ask & Create, Build, Media, Data, Devices & System, and any group you make with the **+** chip; `m` moves the selected tile to another group). The first sign-in
of an administrator adds a few samples (an MCP server, documents, skills). `F1` explains any screen; `o` signs out.

## 2. Connect a model

IYAKKI ships no model. Pick one:

* **Ollama (local or on your network):** install Ollama and `ollama pull` a model. As administrator, IYAKKI finds an Ollama
  on `127.0.0.1:11434` at sign-in and adds it to Settings -> Models. On another machine, open **NET SCAN**, press `s`
  to scan, select the Ollama server and press `a` to add it.
* **OpenAI-compatible server or a cloud provider:** **SETTINGS -> Models**, press `a`, choose the kind (Ollama, OpenAI,
  OpenRouter, Anthropic Claude, Google Gemini, OpenAI-compatible), fill the address (cloud kinds have defaults) and the
  key. Keys go into the encrypted vault, never into a settings file: press `k` on a provider to set or change it.
* Then press `m` on a model row to make it the **chat model**, and `c` to test the chat.

**Expected:** the test answers with text. Models with "thinking" can take a while before the first word.

## 3. Chat

Open **CHAT**. Type a question, Enter to send. `Ctrl+G` switches **Agent** mode on so the assistant can use tools; every
risky call pauses for **Allow once / Allow for this session / Deny** (`Ctrl+Y`, `Ctrl+U`, `Ctrl+N`). Try: "What is the weather in Paris?"
(a read-only lookup, no approval) then "Write a haiku to haiku.txt in my home folder" (asks first).

**Expected:** the answer streams in; the footer shows timing; the write shows an approval card, and after approving the
file exists. Everything is listed in **RUNS**.

## 4. The AI IDE with a sample project

Open **AI IDE**. Press `Ctrl+Shift+W`, choose **New workspace**, name it `hello`. It creates `~/Workspaces/hello` (or a
folder under the data directory in window mode) and runs `git init`. In the chat pane (right) ask:
"Create a tiny Node or Python web server in this folder that answers hello on port 8080, with a Dockerfile."
Approve the file writes. Open a file with `Ctrl+P`. `F1` lists the IDE keys.

**Expected:** the files appear in the tree and open in the editor tabs. `Ctrl+Shift+B` shows the feature board
(reads and writes the workspace's `FEATURES.md`).

## 5. Deploy the hello app to k3s

Needs: `kubectl`, `podman` (or `docker`), and a k3s cluster on this computer whose kubeconfig you can read
([`K3S-DEV.md`](K3S-DEV.md)). IYAKKI never starts or installs a cluster in window mode; it connects to the one
that exists.

1. In the AI IDE press `Ctrl+Shift+K` (cluster view). The header shows the cluster chip (for example
   `k3s · default · reachable`). If more than one cluster answers, **Cluster...** (`c`) lets you choose.
2. Press `d` (**Deploy...**), fill the image name, build folder and port, and confirm.
3. Four steps appear: **Build**, **Get the image to the cluster**, **Apply**, **Check the rollout**. Each shows the real
   command and waits for **Approve** (`y`), **Approve all steps** (`a`) or Cancel (Esc).
4. When the rollout is ready, `Ctrl+Shift+O` opens the **Browser** tab with an "Open app" button for the service.

**Expected:** pods running in namespace `hello-dev`, and the page says hello. When done, **Tear down...** (`T`) stops or
removes it. Pull errors such as "server gave HTTP response to HTTPS client" mean the registry needs a plain-HTTP setting:
press `g` (Registry...) or see [`TROUBLESHOOTING.md`](TROUBLESHOOTING.md#kubernetes-and-deploy).

## 6. A table and a dashboard

Save this as `~/sales.csv` (or any CSV; in window mode drag a file onto the window to import it):

```
day,region,qty,price
2026-10-01,EU,12,9.5
2026-10-01,US,7,10.0
2026-10-02,EU,15,9.5
2026-10-02,US,11,10.0
2026-10-03,EU,9,9.8
```

Open **DATA**, press `o`, choose the file. Click a header to sort; press `/` and type `qty > 10 and region = 'EU'`, Enter;
press `s` for the column summary.

**Expected:** a virtual-scrolled table, a filter that narrows it, a summary with min, max, mean.

Open **DASH** for graphs over Prometheus, Elasticsearch/OpenSearch, ClickHouse or DuckDB (DuckDB: install it from the CLI
tile, then connect it to your CSV). Five dashboard templates are built in. `a` adds a panel; `Ctrl+I` shows the assistant
beside the board.

**Expected:** the board with panels and a time range; without a data source, panels explain what to connect
([`DASHBOARD.md`](DASHBOARD.md)).

**Object storage (S3).** Store, Setup, Cluster: "SeaweedFS (object storage, S3)" and "RustFS (object storage, S3, Rust, young)" install an
S3-compatible store into the cluster (pick one or both). The licence (Apache-2.0) is shown before Install; the keys are generated into your
vault (`svc.seaweedfs.access-key`, `svc.seaweedfs.secret-key`, record `s3.seaweedfs`; the same with `rustfs`). The address works inside the cluster only.
The assistant has `object_storage_status`, `object_storage_install` and `object_storage_remove`; removing keeps the data unless you ask to delete it
([`PLATFORM-SERVICES.md`](PLATFORM-SERVICES.md)).

## 7. Read a story aloud

Needs a text-to-speech engine (for example piper) set in **SETTINGS -> Voice** (`v` to set the TTS command, `y` to test).
Open **TALES**, type an idea (you can say it in words: *a 5-minute funny story in Tamil about a fox*), choose who it is for (Kids, Teens, Adults) and, if you like, the kind and tone, then press `Enter` or **Send**. **Options ▾** has the length, language, characters, pictures and saving; **Model ▾** and **Agent** choose what writes it.

**Expected:** the story appears as it is written, and with a speech engine it is read paragraph by paragraph with a
highlight; `Space` pauses. Without one the story is still written, shown and saved, and the page tells you how to turn
reading on ([`STORY.md`](STORY.md)).

## 8. Install Claude Code from the CLI tile

Needs Node.js 18+ and npm on the computer.
Open **CLI**, press `Tab` to the **CLIs** page, select **Claude Code** and press `i`. IYAKKI shows the exact command
(`npm install -g @anthropic-ai/claude-code`) and the folder it installs into; confirm with `y`. It runs without a shell
and without `sudo`, in a folder under the CLI tile's own data.

**Expected:** the status says it is installed with a version. Press `g` to put the vendor's sign-in command in the
terminal line (it is not run for you), run it in a normal terminal, then in **SETTINGS -> Agents** use Claude Code as
a chat backend (`Ctrl+P` in the chat). Read the terms note first: [`EXTERNAL-AGENTS.md`](EXTERNAL-AGENTS.md#terms-of-service-decision-for-the-owner).

## Removing what Quick install put on the machine

Every item of Store -> Setup can be removed again. Open its tile and press **Remove** (or `r`). A card lists what goes, what stays and, if you tick the box, what is deleted too.

- **Safe by default.** Removing stops the program or the pod and takes away what IYAKKI added (the Deployment and Service, the program and its folder, the Models entry). Your **data stays**: the volume of the item, its database or models, your vault entries and your DASH connections. Installing the item again finds the same volume and the same passwords.
- **Also delete the data** is a tick, off by default. The card says how much it frees and that it cannot be undone. Items that are only their own data (a Linux sandbox, the Kokoro model, the voice pack) are removed only with this tick.
- **Dependants.** k3s and Lemonade cannot be removed while other installed items need them: the card lists them. Tick "Also remove first" to remove them together (their data is kept), or remove them yourself first. Tools that only use another (Node.js, Python) get a warning.
- **Only IYAKKI's own things.** Each target is checked against IYAKKI's namespaces and folders before it is touched; a cluster found on the machine that is not IYAKKI's is never removed.
- **If it stops half-way,** the card says which step failed and offers **Retry**; running a removal twice is harmless.
- **Where the data lives.** The volumes of the cluster are folders under the data volume (`k3s-dev/data/storage`); after an install their reclaim policy is set to Retain, so even deleting a claim by hand does not delete the folder. On a live session without a persistent data volume the card warns that everything is lost at the next restart.
- **From chat:** `quick_install_status` lists the items and `quick_uninstall` removes one. It asks you every time, with the same card, and `delete_data` is false unless you asked for it.

## Where next

[`USER-GUIDE.md`](USER-GUIDE.md) (every tile) · [`FAQ.md`](FAQ.md) · [`ADMIN-GUIDE.md`](ADMIN-GUIDE.md) (accounts, policy, security).
