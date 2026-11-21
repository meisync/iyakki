//! The whole shell over the REAL core's UI socket (F-SPLIT-03 layer 2): `uiserve` serves the real adapters and the roots of the real services on a
//! unix socket; the program side builds its environment with `socket_shell::environment` and runs the shell. Linux only.
#![cfg(all(target_os = "linux", feature = "socket-shell"))]

mod common;


use common::{drive, Device};
use iyakki_agent::contract_local::uiserve::{start_with, Facets, UiSocketConfig};
use iyakki_ui_bin::socket_shell::environment;

#[test]
fn the_shell_draws_signs_in_and_changes_a_setting_through_the_real_cores_socket() {
    let dev = Device::new("real");
    // the core reads the policy of the account it is asked about (D-SEC1-VOUCH), so the plain user the file-scope checks below name must exist
    {
        let mut users = dev.users.lock().unwrap();
        let root = users.authenticate("root", "rootpass12").unwrap();
        users.create_user(&root, "ann", "annpass12345", iyakki_sec::policy::Role::User).unwrap();
    }
    let core = iyakki_agent::contract_local::core_for(&dev.svc);
    let roots = dev.roots();
    let facets = Facets { rpc: Some(roots.factory()), ..Facets::default() };
    let cfg = UiSocketConfig { enabled: true, scopes: vec!["settings:read".into()], account: "ui".into(), admin: false, rpc: true };
    let on = start_with(&core, &cfg, "iyakki-test", &dev.dir.join("on.sock"), &facets).unwrap();
    let env = environment(on.path(), false, 3, &std::thread::yield_now).unwrap();
    // the program's file scope is the core's: the home folder and the refusals are what the core's own rules say
    {
        use iyakki_agent_api::ServicesHandle;
        let policy = iyakki_sec::policy::Role::User.template();
        let data = dev.dir.join("svc");
        let scope = iyakki_apps::file_scope();
        assert_eq!(scope.home_root("ann", &data), ServicesHandle::scope_home_root(&*dev.svc, "ann", &data));
        assert_eq!(scope.roots(&policy, "ann", &data), ServicesHandle::scope_roots(&*dev.svc, &policy, "ann", &data));
        assert!(scope.check_read(&policy, "ann", std::path::Path::new("/etc/shadow")).is_err(), "a path outside the person's reach is refused by the core");
    }
    let svc = env.services.clone();
    let mut sh = iyakki_shell::shell::Shell::new(env);
    drive(&mut sh, &dev);
    let (opened, panicked, unserved) = common::sweep_unserved(&mut sh);
    println!("unserved: {unserved:?}");
    println!("opened {} apps: {opened:?}", opened.len());
    assert!(panicked.is_empty(), "apps that panicked over the socket: {panicked:#?}");
    // every tile of the group is built from objects the core serves: none of them opens as the "not available" screen (FLOWS is the lane of its own)
    for id in ["radio", "media", "lab", "rag", "inbox", "system", "cli", "tales", "store"] {
        assert!(opened.contains(&id) && !unserved.contains(&id), "{id} opens over the socket and is not the unserved screen: unserved {unserved:?}");
    }
    common::tiles_round_trip(&mut sh, &dev, &svc);
    common::tile_apps_round_trip(&mut sh, &dev);
    assert!(opened.len() >= 20, "most apps open: {opened:?}");
}
