//! The wasm32 build over one [`ChatTile`] whose source is the host's `call` import.
use crate::ChatTile;
use iyakki_tile_kit::guest::HostSource;
use iyakki_tile_kit::App;

fn make(src: HostSource) -> Box<dyn App> {
    Box::new(ChatTile::new(src))
}

iyakki_tile_kit::export_tile!(make);
