# IYAKKI on a Mac (Apple silicon)

Three ways, from least to most work. Models run best in **Ollama on the Mac itself** (it uses the GPU through Metal and the
Mac's unified memory); IYAKKI then talks to it over the network.

## 1. The desktop window, natively on macOS  (status: written for, not yet tried)

```
xcode-select --install                                   # compiler and linker
curl https://sh.rustup.rs -sSf | sh                      # Rust
./run.sh window                                          # builds and opens the IYAKKI window
```

The window shows only **virtual disks** (files IYAKKI makes in `~/.local/share/iyakki-dev/disks`, `./run.sh disk add 8G`), never
your Mac's drives, and keeps its accounts and settings in `~/.local/share/iyakki-dev`.

What is expected to work: every tile that is plain code (chat, RAG, editor, browser, calendar, usage, skills, MCP). What is Linux-specific and must degrade cleanly, and has **not been run on a Mac**:

* `/proc` and `/sys` (SYSTEM tile, hardware tiles) are not there on macOS: those pages show zeros or "not available".
* `ip`, `aplay`, `arecord` (network and sound settings, voice, the radio's sound) have no macOS equivalents wired in yet.
* mounting a data disk is Linux-only (the storage code refuses it with a clear message; the virtual disks are never mounted).
* the container cluster (k3s) needs Linux and root, so `sandbox_run` is not available.

If the build fails on your Mac, the first error is the most useful thing to send back: the pieces that touch Linux directly are
listed in `BOUNDARY.md`.

## 2. Models through Ollama on the Mac

1. Install Ollama from <https://ollama.com/download> (the Mac app). It listens on `127.0.0.1:11434`.
2. Pull a model in the Ollama app or with `ollama pull qwen3:8b`.
3. Sign in to IYAKKI as an administrator. IYAKKI looks for an Ollama on `127.0.0.1:11434` at sign-in and adds it to
   Settings → Models as **ollama-local**; choose it and a model for the chat.
4. Pictures: Ollama's image models (for example `x/z-image-turbo`, `x/flux2-klein`) only run on macOS today, so this is also the
   way to use `generate_image` from the chat.

NET SCAN finds Ollama servers elsewhere on your network too.

## 3. IYAKKI in a virtual machine on the Mac

Use the ARM image (`./run.sh build-arm`, then `./run.sh vm-arm`, or UTM with the `iyakki-aarch64.img` disk image). A virtual
machine cannot use the Mac's GPU, so models should still run in the Mac's Ollama:

* With QEMU's default networking the Mac is reachable from the VM as `10.0.2.2`: add a provider in Settings → Models of kind
  Ollama with the address `http://10.0.2.2:11434`.
* With UTM's shared network, use the Mac's own address and start Ollama with `OLLAMA_HOST=0.0.0.0` so it listens beyond the
  loopback (do this only on a network you trust: Ollama has no sign-in).

## Booting a Mac from a USB stick

Not possible on M-series Macs: they only start operating systems that Apple's boot chain accepts. Use the window or a VM.
(Intel Macs can boot the x86-64 ISO like any PC, with the usual caveats about their firmware.)
