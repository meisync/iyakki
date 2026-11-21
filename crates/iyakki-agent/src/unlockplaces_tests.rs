use super::*;

fn scratch(name: &str) -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-places-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_first_place_is_remembered_and_a_second_place_is_counted_once() {
    let d = scratch("count");
    assert!(!note_unlock(&d, "ann", "eth0@0100A8C0"), "the first unlock has nothing to compare with");
    assert!(!note_unlock(&d, "ann", "eth0@0100A8C0"));
    assert_eq!(new_unlocks(&d, "ann"), 0);
    assert!(note_unlock(&d, "ann", "wlan0@010AA8C0"));
    assert!(!note_unlock(&d, "ann", "wlan0@010AA8C0"), "a place seen before is not new again");
    assert_eq!(new_unlocks(&d, "ann"), 1);
    assert_eq!(new_unlocks(&d, "bob"), 0, "each person has a book of their own");
    let text = std::fs::read_to_string(file(&d, "ann")).unwrap();
    assert!(!text.contains("0100A8C0") && !text.contains("eth0"), "no address is kept");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_machine_without_a_route_records_nothing_and_the_default_route_names_the_place() {
    let d = scratch("none");
    assert!(!note_unlock(&d, "ann", ""));
    assert!(!file(&d, "ann").exists());
    let table = "Iface\tDestination\tGateway\tFlags\nwlan0\t00000000\t0100A8C0\t0003\nwlan0\t0000A8C0\t00000000\t0001\n";
    assert_eq!(place_from_routes(table), "wlan0@0100A8C0");
    assert_eq!(place_from_routes("Iface\tDestination\n"), "");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_book_keeps_only_the_newest_thirty_two_places() {
    let d = scratch("cap");
    for i in 0..40 {
        note_unlock(&d, "ann", &format!("net{i}"));
    }
    let b: Book = serde_json::from_str(&std::fs::read_to_string(file(&d, "ann")).unwrap()).unwrap();
    assert_eq!(b.places.len(), 32);
    assert_eq!(b.new_unlocks, 39);
    let _ = std::fs::remove_dir_all(&d);
}
