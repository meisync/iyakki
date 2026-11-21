//! The wasm32 build: the six exports of tile ABI 3 over CLI, with the core's terminal, storage and dock behind the tile's one door (`call`).
//! The host checks the manifest's capabilities before any of it is forwarded. The tile declares ABI 3 (the `tile` feature turns
//! `iyakki-sdk-tile/app-events` on), so the shell sends it the pinch that changes the text size (D-SDK-34-01 (a)).
use crate::CliApp;
use iyakki_sdk_tile::ports::{CoreDock, CoreStorage, CoreTerminal, Shared};
use iyakki_sdk_tile::{kit, SdkTile};

fn make(src: kit::guest::HostSource) -> Box<dyn kit::App> {
    let door = Shared::new(src);
    let app = CliApp::new(Box::new(CoreTerminal::new(door.clone()))).with_storage(Box::new(CoreStorage::new(door.clone())));
    Box::new(SdkTile::new(app, Box::new(CoreDock::new(door))))
}

kit::export_tile!(make);
