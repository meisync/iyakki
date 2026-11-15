# WASM snapshots: copy-on-write forks and a snapshot in the middle of a call (F-WASM-06, F-WASM-07)

**For:** whoever picks these two rows up.
**You will get:** what the engine can and cannot do today, why neither row can be finished honestly with the current design, the smallest steps that are real, and what would make each one doable. No code was written for these rows; both stay `planned`.

## What exists

`iyakki-wasm` is a thin layer over **Wasmtime** (not an engine of our own). A `Session` owns one instance. `snapshot()` copies the whole linear
memory (`Vec<u8>`), the exported mutable globals and the VFS into a `Snapshot`; `restore()` writes the memory back (zeroing what lies beyond the
snapshot) and `fork()` makes a new instance and restores the snapshot into it. The note in the source says it plainly: "Call stacks cannot be frozen
mid-call; snapshots are taken between calls." Cost today: a 64 MiB guest costs a 64 MiB copy for every snapshot and every fork.

## F-WASM-06: copy-on-write snapshots with sub-millisecond forks

**Why it cannot be done honestly now.** A fork must start with the parent's memory without copying it. Wasmtime offers that only for a module's
*initial* memory image (`memory_init_cow`, with the pooling allocator): the image comes from the module's data segments, not from a running
instance. Three routes to the same effect, none small:

| Route | What it needs | Why it is not taken now |
|---|---|---|
| Rewrite the snapshot into a new module (the live memory as one data segment) and instantiate that with Wasmtime's copy-on-write image | Build a module per snapshot: validation and compilation work, cached engine settings (pooling, `memory_init_cow`) | The module-building cost is milliseconds and the module must be kept for the life of every fork; the exported-global and table state would have to go into the module too |
| Page protection or `userfaultfd` on the instance's memory, copying a page when the guest first writes it | Linux-only kernel features and access to the memory's mapping, which Wasmtime owns | Cannot be tested on the Mac build machines, needs unsafe code around memory Wasmtime manages, and a new row in the kernel-access module |
| An engine of our own with a paged memory (page table of shared pages) | Replacing Wasmtime's code generator for guests | Months, and it slows every load and store |

**What is real and could be built without those:** a *paged* snapshot (`Snapshot` holds `Arc` pages of 64 KiB, identical pages shared between a snapshot and
its children, found by comparing content) so that *storing* many snapshots of one guest costs only the changed pages, and `restore()` writes only the pages
that differ from the live memory. That reduces memory for a tree of branches (the agent's what-if runs in `branches`) and restore time when little changed.
It is not sub-millisecond for a 64 MiB guest (comparing 1024 pages still reads the whole memory, a few milliseconds), so it would be a new row
("shared pages between snapshots"), not a completion of this one.

**What would let the row be finished:** measure first. Time `fork()` at 1, 16 and 64 MiB on the Linux build machine and on the Mac; if the memory copy is not
the cost the agent runs actually feel (module instantiation and VFS fork usually are), the row's target ("sub-millisecond") should be changed to a measured
budget in F-PKG-05's budget file instead of chasing the copy.

## F-WASM-07: a snapshot in the middle of a guest call

**What it means.** The guest is stopped inside a call (out of fuel, or waiting for a host function), its stack is saved, and later (or in a fork) it
continues from the same instruction.

**Why it cannot be done with Wasmtime.** The WebAssembly call stack is not part of linear memory and is not readable or writable through any Wasmtime
API; a stopped call can only be continued by the same store on the same native stack (or fiber). A fiber cannot be copied. So there are only two real
meanings:

1. **Suspend and resume in the same process, same instance.** Run the guest on Wasmtime's async support (a fiber), let a host call or the fuel-yield
   (`fuel_async_yield_interval`) hand control back, keep the store alive, and continue it later. That is *pausing*, not a snapshot: it cannot be saved to a
   file, moved to another process or forked. It is buildable, and it needs the callers (`Session::call`) to become asynchronous.
2. **Unwinding by transformation.** A build step instruments the guest module (the Asyncify technique: every function can save its locals into memory and
   return, and be re-entered to rebuild the stack), after which the state *is* in linear memory and the existing snapshot covers it. That needs a module
   rewriter (a few thousand lines, since we do not take dependencies we can avoid) and costs 30 to 50 percent run time on instrumented modules. It is the only
   way to a serialisable, forkable mid-call snapshot.

**Recommendation.** Keep both rows `planned`. If the agent flow needs "stop a long guest run, look, and carry on", build meaning 1 as its own row (it
is about a week of work and needs no new technology). Choose meaning 2 only if a flow must save a half-finished call to disk or fork it; decide that with the
owner, because it adds a build step to every WASM tile.

## What was checked

Reading the source of `crates/iyakki-wasm/src/lib.rs` (snapshot, restore, fork, `branches`), and the Wasmtime sources in the cargo registry for the
version `Cargo.lock` pins: **wasmtime 49.0.2**, built by `Cargo.toml` with `default-features = false, features = ["runtime", "cranelift", "wat", "std"]`.
Facts read there (2026-11-12):

* `Config::memory_init_cow(bool)` exists and is on by default. Its documentation limits it to the *initial memory image of a module*: on Linux it
  maps the module's data segments copy-on-write (from the compiled file, or a `memfd_create` image); on other Unix systems only for a module loaded with
  `Module::deserialize_file`; Windows copies. It is not a way to start an instance from another instance's live memory, which is what a fork needs.
* `Store::fuel_async_yield_interval` exists, but the `async` feature (which pulls in `wasmtime-fiber`) is **not** enabled in this workspace. So the
  "pause on a fiber" meaning of F-WASM-07 would first need that feature added, and `Session::call` made asynchronous.
* The only stack-reading facility is the core dump captured on a trap (`coredump_on_trap`): it is a read-only report of a trapped guest, not something a
  guest can be resumed from. No API reads or writes a guest's call stack.

These facts agree with the two sections above (they were written before this check from general knowledge). Nothing was measured and no code was run
for this note.
