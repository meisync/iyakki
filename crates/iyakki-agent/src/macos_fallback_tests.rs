//! The macOS fallbacks (F-SHELL-20), tested only where they are in force: on any system but Linux. The Linux-only calls fail with the
//! words the person sees, the `/proc` and `/sys` readers give empty values and never panic, and the process helpers use `ps`. On Linux
//! this file compiles to nothing; its tests are proved by running the agent's tests on a Mac.
#![cfg(all(test, not(target_os = "linux")))]

use crate::oscompat;
use std::ffi::CString;

#[test]
fn linux_only_system_calls_fail_with_enosys_and_the_pipe_size_commands_are_refused() {
    let p = CString::new("/nonexistent").unwrap();
    let errno = || std::io::Error::last_os_error().raw_os_error();
    assert_eq!(unsafe { oscompat::mount(std::ptr::null(), p.as_ptr(), std::ptr::null(), oscompat::MS_BIND, std::ptr::null()) }, -1);
    assert_eq!(errno(), Some(libc::ENOSYS), "mount");
    assert_eq!(unsafe { oscompat::umount2(p.as_ptr(), oscompat::MNT_DETACH) }, -1);
    assert_eq!(errno(), Some(libc::ENOSYS), "umount2");
    // SAFETY: an all-zero sched_param is valid; the call fails before it reads it
    let sp: libc::sched_param = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { oscompat::sched_setscheduler(0, libc::SCHED_RR, &sp) }, -1);
    assert_eq!(errno(), Some(libc::ENOSYS), "sched_setscheduler");
    // the pipe-size commands are not commands of this system: the call fails and the callers keep the default size
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let set = unsafe { libc::fcntl(fds[1], oscompat::F_SETPIPE_SZ, 65536) };
    let get = unsafe { libc::fcntl(fds[1], oscompat::F_GETPIPE_SZ) };
    unsafe {
        libc::close(fds[0]);
        libc::close(fds[1]);
    }
    assert_eq!((set, get), (-1, -1));
}

#[test]
fn the_slave_name_of_a_real_pseudo_terminal_is_found_without_the_linux_call() {
    unsafe {
        let m = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(m >= 0, "a pseudo-terminal master");
        assert_eq!(libc::grantpt(m), 0);
        assert_eq!(libc::unlockpt(m), 0);
        let name = oscompat::pts_name(m).expect("the slave name");
        assert!(name.to_string_lossy().starts_with("/dev/tty"), "{name:?}");
        libc::close(m);
        assert!(oscompat::pts_name(-1).is_err(), "a bad descriptor is an error, not a panic");
    }
}

#[test]
fn features_that_need_linux_say_so_in_the_same_words_and_open_nothing() {
    let said = |e: String| assert!(e.contains(oscompat::UNAVAILABLE) || e.contains("Linux"), "{e}");
    said(format!("{:?}", crate::vpn_sys::NlTransport::open().err().expect("no WireGuard setup here")));
    said(format!("{:?}", crate::nl80211_os::Nl80211Socket::open(std::time::Duration::from_millis(10)).err().expect("no nl80211 here")));
    said(crate::nl80211_os::PacketEapol::open(1).err().expect("no EAPOL socket here"));
    said(crate::nl80211_os::udp_on_interface("wlan0", 6767).err().expect("no per-interface socket here"));
}

#[test]
fn the_proc_and_sys_readers_give_empty_values_and_never_panic() {
    use crate::sysinfo::*;
    let r = SysRoots::real();
    assert!(!r.proc.exists() || !r.proc.join("stat").exists(), "this system has no Linux /proc/stat");
    assert_eq!(cpu_times(&r), CpuTimes::default());
    assert_eq!(cpu_count(&r), 1, "at least one");
    assert_eq!(memory(&r), Memory::default());
    assert_eq!(load_average(&r), (0.0, 0.0, 0.0));
    assert_eq!(uptime_secs(&r), 0);
    assert!(hottest_temp(&r).is_none());
    assert!(net_counters(&r).is_empty() && disk_counters(&r).is_empty() && processes(&r).is_empty());
    assert_eq!(product_name(&r.sys), "");
}

#[test]
fn process_helpers_use_ps_for_this_process_and_know_a_missing_one() {
    let me = std::process::id();
    assert!(crate::jobs::process_exists(me));
    assert!(crate::jobs::process_name(me).is_some_and(|n| !n.is_empty()), "ps names this process");
    // a process id that no system hands out
    assert!(!crate::jobs::process_exists(0x7fff_fff0));
    assert!(crate::jobs::process_name(0x7fff_fff0).is_none());
}
