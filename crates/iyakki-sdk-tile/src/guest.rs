//! The wasm32 build: the exports of tile ABI 1 over the SAMPLE app as an SDK tile, its notes kept and its dock used through the core.
use iyakki_app_sample::SampleApp;

crate::export_sdk_tile!(SampleApp::with_storage);
