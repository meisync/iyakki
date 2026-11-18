#[test]
fn the_unpack_limit_of_the_catalogue_is_the_archive_readers_limit() {
    assert_eq!(iyakki_shared::clis::MAX_UNPACKED, iyakki_archive::archive::MAX_TOTAL);
}
