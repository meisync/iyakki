//! Types moved out of `iyakki_agent::settingstools_wasmtiles` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nWASM TILES (Settings -> Developer mode): wasm_tiles_status shows whether tile apps that carry a WebAssembly module may run (experimental, off by default; free). wasm_tiles_set turns that on or off and asks first (administrators only). A module runs in the tile sandbox with time, fuel and memory limits and gets no tools or files.";
