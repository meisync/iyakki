use super::*;
use iyakki_web::net::{GIB, MIB};

fn m(avail: u64, total: u64, cores: usize) -> Machine {
    Machine { mem_total: total, mem_available: avail, disk_free: 50 * GIB, cores }
}

#[test]
fn worker_counts_follow_cores_and_shrink_when_memory_is_short() {
    assert_eq!(preload_workers(&m(4 * GIB, 8 * GIB, 1)), 4);
    assert_eq!(preload_workers(&m(4 * GIB, 8 * GIB, 4)), 8);
    assert_eq!(preload_workers(&m(4 * GIB, 8 * GIB, 64)), 12);
    assert_eq!(preload_workers(&m(300 * MIB, 2 * GIB, 8)), 3);
    assert_eq!(image_workers(&m(4 * GIB, 8 * GIB, 1)), 4);
    assert_eq!(image_workers(&m(4 * GIB, 8 * GIB, 6)), 6);
    assert_eq!(image_workers(&m(4 * GIB, 8 * GIB, 64)), 8);
    assert_eq!(image_workers(&m(200 * MIB, 2 * GIB, 8)), 2);
    // unknown memory (0 total) keeps the normal counts
    assert_eq!(image_workers(&m(0, 0, 4)), 4);
}

#[test]
fn the_budget_follows_the_machine_and_the_choice() {
    let big = budget_for(CacheSize::Auto, &m(8 * GIB, 16 * GIB, 8));
    let tight = budget_for(CacheSize::Auto, &m(300 * MIB, 2 * GIB, 8));
    assert!(tight.mem < big.mem);
    assert!(budget_for(CacheSize::Large, &m(8 * GIB, 16 * GIB, 8)).mem >= big.mem);
    assert_eq!(budget_for(CacheSize::Auto, &m(50 * MIB, 2 * GIB, 8)), Budget::OFF);
}

#[test]
fn the_gate_refreshes_at_most_once_per_interval_and_hands_the_budget_to_the_transport() {
    struct Spy(std::sync::Mutex<Vec<Budget>>);
    impl iyakki_web::Fetch for Spy {
        fn get(&self, _: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
            Err("no".into())
        }
        fn set_cache_budget(&self, b: Budget) {
            self.0.lock().unwrap().push(b);
        }
    }
    let spy = Spy(Default::default());
    let mut g = Gate::new(Duration::from_secs(3600));
    assert!(g.refresh(&spy, Path::new("/"), CacheSize::Auto).is_some());
    assert!(g.refresh(&spy, Path::new("/"), CacheSize::Auto).is_none());
    g.invalidate();
    assert!(g.refresh(&spy, Path::new("/"), CacheSize::Small).is_some());
    assert_eq!(spy.0.lock().unwrap().len(), 2);
}

#[test]
fn the_numbers_read_plainly() {
    let l = numbers(None, net::Snapshot::default());
    assert_eq!(l[0].0, "Cache");
    assert!(l[1].1.contains("no requests yet"));
    let c = net::Snapshot { mem_hits: 6, disk_hits: 2, revalidated: 2, misses: 10, preload_used: 5, preload_wasted: 1, bytes_in: 3 << 20, ..Default::default() };
    let l = numbers(Some(iyakki_web::cache::CacheStats { mem_bytes: 2 << 20, mem_entries: 4, disk_bytes: 5 << 20, disk_entries: 9, budget: Budget { mem: 8 << 20, disk: 64 << 20, max_entry: 1 << 20 } }), c);
    assert!(l[0].1.contains("2.0 MB used of 8.0 MB"));
    assert!(l[1].1.contains("9 files"));
    assert!(l[2].1.contains("50% of 20 requests"), "{}", l[2].1);
}
