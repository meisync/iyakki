//! The wasm32 build: the exports of tile ABI 1 over one [`SettingsTile`] whose source is the host's `call` import.
use crate::SettingsTile;
use iyakki_tile_kit::guest::HostSource;
use iyakki_tile_kit::App;

fn make(src: HostSource) -> Box<dyn App> {
    Box::new(SettingsTile::new(src))
}

iyakki_tile_kit::export_tile!(make);
