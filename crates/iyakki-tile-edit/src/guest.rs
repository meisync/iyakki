//! The wasm32 build over one [`EditTile`] whose source is the host's `call` import.
use crate::EditTile;
use iyakki_tile_kit::guest::HostSource;
use iyakki_tile_kit::App;

fn make(src: HostSource) -> Box<dyn App> {
    Box::new(EditTile::new(src))
}

iyakki_tile_kit::export_tile!(make);
