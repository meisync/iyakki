//! Types moved out of `iyakki_agent::settingstools_startup` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nSTARTUP DISK (Settings -> This computer): startup_disk_status says what the firmware starts first and whether IYAKKI has its own entry in the boot menu (free). startup_disk_register makes IYAKKI's own boot entry for the volume it is running from (a stick that starts through the removable-media path has none, so a dual-boot computer goes to Windows); startup_disk_set puts IYAKKI first (`first`), starts it at the next restart only (`next`), puts the old order back (`revert`) or switches `reboot_into_iyakki` on or off (restarts through `iyakki dev reboot` then start IYAKKI once). Each asks first, administrators only; these write firmware boot variables, so never call them unless the person asked for that.";
