//! How the headless web service image starts (F-WEBSVC-15). The image has no shell and no screen: its init program is the headless `iyakki`, started
//! by the kernel with no arguments as process 1, so the choice "what do I serve" cannot come from a command line. The profile comes from, in
//! this order: `serve --profile NAME` / `IYAKKI_PROFILE` (a container, a test), the kernel line `iyakki.profile=NAME` (and `iyakki.listen=ADDR`), or
//! the `profile` of the service file `/etc/iyakki/service.json` that the image carries (the cloud profile). The pure decisions are here and are
//! unit-tested; `main.rs` does the booting (pseudo file systems, drivers, data volume, network) and the serving.

use crate::svc::Ask;

/// Whether this process is the init of a real boot: process 1 and nothing has mounted /proc yet (a container's process 1 already has /proc and
/// must never mount file systems or load drivers).
pub fn is_real_boot(pid: u32, proc_self_exists: bool) -> bool {
    pid == 1 && !proc_self_exists
}

/// What the headless image should serve, or `None` when nothing was asked for and the image has no service file (then `serve` says how to ask).
/// `config_present`: the service file of the image exists. `args` are the words after `serve`.
pub fn image_ask(args: &[String], env: &dyn Fn(&str) -> Option<String>, cmdline: &str, config_present: bool) -> Result<Option<Ask>, String> {
    if let Some(a) = crate::svc::parse_args(args, env)? {
        return Ok(Some(a));
    }
    let word = |k: &str| crate::boot::cmdline_param(cmdline, k).filter(|v| !v.is_empty()).map(String::from);
    let profile = word("iyakki.profile");
    if profile.is_none() && !config_present {
        return Ok(None);
    }
    Ok(Some(Ask { profile, config: env("IYAKKI_SERVICE_CONFIG").filter(|p| !p.is_empty()), listen: word("iyakki.listen").or_else(|| env("IYAKKI_LISTEN").filter(|p| !p.is_empty())), check_only: args.iter().any(|a| a == "--check") }))
}

/// A service started as process 1 must never return: when it ends (a signal, a failure) the machine powers off instead of the kernel stopping on
/// "attempted to kill init".
pub fn after_service_ends(real_boot: bool) -> AfterService {
    if real_boot {
        AfterService::PowerOff
    } else {
        AfterService::Exit
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum AfterService {
    PowerOff,
    Exit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_real_boot_is_process_1_without_proc_and_a_container_is_not() {
        assert!(is_real_boot(1, false));
        assert!(!is_real_boot(1, true), "a container's init already has /proc");
        assert!(!is_real_boot(4242, false));
    }

    #[test]
    fn the_image_with_its_service_file_serves_with_no_arguments_at_all() {
        let a = image_ask(&[], &no_env, "quiet ro", true).unwrap().expect("the file says what to serve");
        assert_eq!(a, Ask { profile: None, config: None, listen: None, check_only: false });
        // the same for a boot that was given `serve` and nothing else
        assert!(image_ask(&s(&[]), &no_env, "", true).unwrap().is_some());
    }

    #[test]
    fn nothing_asked_and_no_file_means_nothing_to_serve() {
        assert_eq!(image_ask(&[], &no_env, "quiet", false).unwrap(), None);
    }

    #[test]
    fn the_kernel_line_names_the_profile_and_the_listen_address_and_beats_nothing_on_the_command_line() {
        let a = image_ask(&[], &no_env, "quiet iyakki.profile=data-api iyakki.listen=0.0.0.0:9443 ro", false).unwrap().unwrap();
        assert_eq!((a.profile.as_deref(), a.listen.as_deref()), (Some("data-api"), Some("0.0.0.0:9443")));
        // an empty value is no value
        let a = image_ask(&[], &no_env, "iyakki.profile= iyakki.listen=", true).unwrap().unwrap();
        assert_eq!((a.profile, a.listen), (None, None));
    }

    #[test]
    fn a_profile_on_the_command_line_or_in_the_environment_wins_over_the_kernel_line() {
        let a = image_ask(&s(&["--profile", "flows-runner"]), &no_env, "iyakki.profile=data-api", true).unwrap().unwrap();
        assert_eq!(a.profile.as_deref(), Some("flows-runner"));
        let env = |k: &str| (k == "IYAKKI_PROFILE").then(|| "agent-api".to_string());
        let a = image_ask(&[], &env, "iyakki.profile=data-api", true).unwrap().unwrap();
        assert_eq!(a.profile.as_deref(), Some("agent-api"));
        assert!(image_ask(&s(&["--profile"]), &no_env, "", true).is_err(), "a flag without a value is an error, not a silent default");
    }

    #[test]
    fn check_and_the_config_path_are_kept_for_an_image_start() {
        let env = |k: &str| (k == "IYAKKI_SERVICE_CONFIG").then(|| "/etc/iyakki/other.json".to_string());
        let a = image_ask(&s(&["--check"]), &env, "", true).unwrap().unwrap();
        assert!(a.check_only);
        assert_eq!(a.config.as_deref(), Some("/etc/iyakki/other.json"));
    }

    #[test]
    fn a_service_that_ends_as_process_1_powers_the_machine_off_and_anywhere_else_just_exits() {
        assert_eq!(after_service_ends(true), AfterService::PowerOff);
        assert_eq!(after_service_ends(false), AfterService::Exit);
    }

    #[test]
    fn the_cloud_service_file_of_the_image_is_a_complete_answer_to_what_to_serve() {
        // the file the image installs as /etc/iyakki/service.json names its own profile, so an image start needs nothing else
        let cfg = iyakki_agent::webservice::config::ServiceConfig::parse(include_str!("../../../dist/service/service.cloud.json")).unwrap();
        assert_eq!(cfg.check().unwrap().profile, iyakki_agent::webservice::Profile::Cloud);
        let ask = image_ask(&[], &no_env, "quiet", true).unwrap().unwrap();
        assert_eq!(ask.profile, None, "the file's own profile is kept");
    }
}
