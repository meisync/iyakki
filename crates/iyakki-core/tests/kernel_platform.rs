//! Platform options the x86-64 kernel needs on current laptops. Found on the HP Pavilion 16-af0 (Intel Meteor Lake, Insyde firmware,
//! 2026-11-07): the loader handed the kernel over and nothing followed, stage 1 never ran. The firmware of such machines enables x2APIC
//! and can lock the legacy xAPIC off; a kernel built without `CONFIG_X86_X2APIC` then switches the APIC off entirely ("Kernel does not
//! support x2APIC, please recompile", an error line that `quiet` hides) and the start dies before any screen output.

use std::path::Path;

fn fragment() -> String {
    let rel = "dist/buildroot/board/iyakki/linux.fragment";
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_x86_kernel_supports_x2apic_with_interrupt_remapping() {
    let t = fragment();
    for want in ["CONFIG_X86_X2APIC=y", "CONFIG_IRQ_REMAP=y", "CONFIG_INTEL_IOMMU=y"] {
        assert!(t.lines().any(|l| l == want), "linux.fragment lacks {want} (needed on Meteor Lake and newer Intel laptops)");
    }
    assert!(!t.lines().any(|l| l == "# CONFIG_X86_X2APIC is not set"));
}

#[test]
fn the_hp_plan_records_the_x2apic_finding() {
    let d = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/development/HP-LAPTOP-PLAN.md")).unwrap();
    assert!(d.contains("x2APIC"), "the HP plan must say what the first boot log showed and the x2APIC cause");
}
