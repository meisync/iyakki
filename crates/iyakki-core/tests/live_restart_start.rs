//! The start of the program splits into steps done once per boot and steps done once per process (F-SPLIT-06, first slice). The program of
//! a live restart is process 1 with /proc mounted, so it is not "pid1" in the sense of `main.rs`; it must run the per-process steps and
//! none of the boot steps. This reads `src/main.rs` (no guest needed) and keeps the two groups apart.

const MAIN: &str = include_str!("../src/main.rs");

fn pos(needle: &str) -> usize {
    MAIN.find(needle).unwrap_or_else(|| panic!("main.rs has no `{needle}`"))
}

#[test]
fn the_boot_steps_stay_in_the_pid1_block_and_the_per_process_steps_follow_in_a_block_a_live_restart_enters() {
    let boot_block = pos("            if pid1 {\n                // /proc must exist before the command line can be read");
    let process_block = pos("            if pid1 || restarted {");
    assert!(boot_block < process_block, "the boot block comes first");
    // every boot step is between the two markers
    for step in ["mount_pseudo_filesystems()", "early_boot_critical()", "late_boot(plan)", "run_install_console()"] {
        let at = pos(step);
        assert!(at > boot_block && at < process_block, "`{step}` belongs to the boot part (inside `if pid1`)");
        assert_eq!(MAIN.matches(step).count(), MAIN[boot_block..process_block].matches(step).count(), "`{step}` is not called anywhere else");
    }
    // the per-process steps come after the marker, so a restarted program does them
    let tail = &MAIN[process_block..];
    for step in ["start_shell_thread();", "mark_boot_good", "iyakki-webtest", "iyakki-live-restart"] {
        assert!(tail.contains(step), "`{step}` is a per-process step: it must be in the block a live restart enters");
    }
    assert!(!MAIN[boot_block..process_block].contains("start_shell_thread();"), "the shell thread is not a boot step");
}

#[test]
fn a_restarted_program_is_only_recognised_when_it_is_process_one_with_proc_mounted_and_has_a_state_file_for_it() {
    let line = MAIN.lines().find(|l| l.contains("let live_start = if !pid1")).expect("the live start lookup");
    assert!(line.contains("std::process::id() == 1") && line.contains("liverestart::begin"), "{line}");
    // `begin` answers only for a state file naming this very process id and the phase `exec` (tested in liverestart.rs)
}
