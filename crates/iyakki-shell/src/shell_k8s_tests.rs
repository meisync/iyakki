//! The K8S tile on the home screen: in the System group, for administrators, operators and users (the roles that have the AI IDE and
//! so its cluster tab), not for guests, remote-safe in Cloud mode, and opening the app. The shell's tests never reach a cluster: the
//! tile gets the offline rig there.

use super::tests::*;
use super::*;
use iyakki_agent::cloudmode::{self, Class, Facts, Setting};
use iyakki_sec::policy::Role;

#[test]
fn the_k8s_tile_is_registered_once_in_the_system_group_for_the_roles_that_have_the_ide() {
    assert_eq!(TILES.iter().filter(|t| t.app == PApp::K8s).count(), 1);
    let t = TILES.iter().find(|t| t.app == PApp::K8s).unwrap();
    assert_eq!((t.label, t.sub), ("K8S", "Explore the cluster"));
    assert_eq!(Group::of(PApp::K8s), Some(Group::System));
    assert_eq!(PApp::K8s.id(), "k8s");
    assert_eq!(PApp::K8s.label(), "K8S");
    for r in [Role::Admin, Role::Operator, Role::User] {
        assert!(r.template().apps.contains(&PApp::K8s), "{r:?}: the IDE's cluster tab is for them, so the K8S tile is too");
        assert!(r.template().apps.contains(&PApp::Ide), "{r:?}");
    }
    assert!(!Role::Guest.template().apps.contains(&PApp::K8s), "guests get neither the IDE nor a cluster");
    assert_eq!(cloudmode::app_class(PApp::K8s), Class::Remote, "cluster tools are remote-safe");
    assert_eq!(crate::memgate::class_of(PApp::K8s), crate::memgate::AppClass::Light);
    let forced = cloudmode::cloud_mode(Setting::On, &Facts::full());
    assert!(!forced.hides_app(PApp::K8s));
    let bare = cloudmode::cloud_mode(Setting::Auto, &Facts::bare());
    assert!(!bare.hides_app(PApp::K8s), "no sound card, no camera: the K8S tile stays");
}

#[test]
fn the_tile_shows_for_each_role_and_opens_the_app() {
    for (user, pass, shown) in [("root", "rootpass12", true), ("oscar", "operpass12", true), ("alice", "alicepass1", true), ("gus", "guestpass1", false)] {
        let (mut sh, _) = shell_with_users(&format!("k8s-tile-{user}"));
        login(&mut sh, user, pass);
        let labels: Vec<&str> = sh.visible_tiles().iter().map(|t| t.label).collect();
        let has = labels.contains(&"K8S");
        // a guest is limited to the apps of its policy: the tile is listed but opening it is refused by the policy
        if shown {
            assert!(has, "{user}");
            sh.set_group(Group::System);
            assert!(sh.group_tiles().iter().any(|t| t.app == PApp::K8s), "{user}: in the System group");
            let i = sh.group_items().iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::K8s)).unwrap();
            sh.sel = i;
            sh.handle(&Event::Key(Key::Enter));
            assert_eq!(sh.screen_name(), "app:k8s", "{user}");
            let mut g = Gfx::new(1280, 720);
            sh.render(&mut g);
            // the offline rig: the dev cluster is not set up, so the tile says so and asks nothing of any cluster
            sh.handle(&Event::Key(Key::Esc));
            assert_eq!(sh.screen_name(), "launcher", "Esc leaves the tile");
        } else {
            assert!(!sh.session.as_ref().unwrap().policy.allows_app(PApp::K8s), "{user}");
        }
    }
}

#[test]
fn the_tile_draws_inside_the_shell_at_the_supported_sizes() {
    let (mut sh, _) = shell_with_users("k8s-tile-sizes");
    login(&mut sh, "root", "rootpass12");
    sh.set_group(Group::System);
    let i = sh.group_items().iter().position(|it| matches!(it, LItem::Sys(t) if t.app == PApp::K8s)).unwrap();
    sh.sel = i;
    sh.handle(&Event::Key(Key::Enter));
    for (w, h) in [(800u32, 480u32), (1280, 720), (1920, 1080), (360, 740)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        assert!(g.rgba().iter().any(|b| *b != 0), "{w}x{h}");
    }
}
