//! The wasm32 build: the six exports of tile ABI 3 over EDIT, with the core's files, assistant and storage and the core's dock behind the
//! tile's one door (`call`). The host checks the manifest's capabilities before any of it is forwarded. The tile declares ABI 3 (the
//! `tile` feature turns `iyakki-sdk-tile/app-events` on), so the shell sends it the pinch that changes the text size (D-SDK-34-01 (a)).
use crate::EditApp;
use iyakki_sdk_tile::ports::{CoreAssistant, CoreDock, CoreFiles, CoreStorage, Shared};
use iyakki_sdk_tile::{kit, SdkTile};

fn make(src: kit::guest::HostSource) -> Box<dyn kit::App> {
    let door = Shared::new(src);
    let app = EditApp::new(Box::new(CoreFiles::new(door.clone()))).with_assistant(Box::new(CoreAssistant::new(door.clone()))).with_storage(Box::new(CoreStorage::new(door.clone())));
    Box::new(SdkTile::new(app, Box::new(CoreDock::new(door))))
}

kit::export_tile!(make);
