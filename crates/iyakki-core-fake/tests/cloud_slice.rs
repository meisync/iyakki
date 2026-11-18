//! The fake's side of the cloud-mode facet (F-SPLIT-03): a desktop until a test sets another mode.

use iyakki_cloudmode::{cloud_mode, Facts, Setting};
use iyakki_core_api::Core;
use iyakki_core_fake::FakeCore;

#[test]
fn the_fake_core_is_a_desktop_until_a_test_sets_a_cloud_mode() {
    let core = FakeCore::default();
    assert!(!core.cloud().detect().on);
    let bare = cloud_mode(Setting::Auto, &Facts::bare());
    assert!(bare.on);
    core.cloud_for_test().set_for_test(bare.clone());
    assert_eq!(core.cloud().detect(), bare);
}

#[test]
fn the_fake_file_scope_follows_the_policy_and_records_what_was_asked() {
    use iyakki_sec::policy::{Policy, Role};
    let core = FakeCore::default();
    let admin = Policy::resolve(Role::Admin, &Default::default());
    let guest = Policy::resolve(Role::Guest, &Default::default());
    let p = std::path::Path::new("/etc/hostname");
    assert_eq!(core.file_scope().check_read(&admin, "root", p), Ok(()));
    assert!(core.file_scope().check_write(&guest, "visitor", p).is_err(), "a guest may not write");
    assert_eq!(core.file_scope_for_test().asked(), ["check_read(root,/etc/hostname)", "check_write(visitor,/etc/hostname)"]);
    assert!(core.file_scope().home_root("amy", std::path::Path::new("/data")).ends_with("home/amy"));
}

#[test]
fn the_fake_host_answers_what_a_test_set_and_nothing_else() {
    use iyakki_core_api::sysinfo::Memory;
    let core = FakeCore::default();
    assert_eq!((core.host().cmdline(), core.host().mem_total_kb(), core.host().uptime_secs()), (String::new(), 0, 0));
    core.host_fake().set_cmdline("quiet iyakki.mode=cloud");
    core.host_fake().set_memory(Memory { total: 8 << 30, available: 1 << 30, swap_total: 0, swap_free: 0 });
    core.host_fake().set_product("Mac mini");
    assert_eq!(core.host().cmdline(), "quiet iyakki.mode=cloud");
    assert_eq!(core.host().mem_total_kb(), 8 << 20);
    assert_eq!(core.host().memory().used(), 7 << 30);
    assert_eq!(core.host().product_name(), "Mac mini");
}

#[test]
fn the_default_host_answers_empty_figures_for_the_system_page() {
    use iyakki_core_api::sysinfo::SysRoots;
    let core = FakeCore::default();
    let r = SysRoots::real();
    assert_eq!(core.host().cpu_times(&r), Default::default());
    assert!(core.host().processes(&r).is_empty() && core.host().net_counters(&r).is_empty() && core.host().disk_counters(&r).is_empty());
    assert_eq!(core.host().load_average(&r), (0.0, 0.0, 0.0));
    assert!(core.host().signal(2, false).is_err(), "a core that does not serve the facet refuses to signal");
}
