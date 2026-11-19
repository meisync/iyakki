use super::*;
use std::mem::{offset_of, size_of};

#[test]
fn the_structs_have_the_kernels_sizes_and_field_offsets() {
    // from include/uapi/drm/drm_mode.h of Linux 6.12 (x86-64 and arm64 agree: every field is naturally aligned)
    assert_eq!(size_of::<ModeInfo>(), 68);
    assert_eq!(offset_of!(ModeInfo, vrefresh), 24);
    assert_eq!(offset_of!(ModeInfo, name), 36);
    assert_eq!(size_of::<CardRes>(), 64);
    assert_eq!(offset_of!(CardRes, count_fbs), 32);
    assert_eq!(size_of::<GetConnector>(), 80);
    assert_eq!(offset_of!(GetConnector, count_modes), 32);
    assert_eq!(offset_of!(GetConnector, connection), 60);
    assert_eq!(size_of::<GetEncoder>(), 20);
    assert_eq!(size_of::<CrtcCmd>(), 104);
    assert_eq!(offset_of!(CrtcCmd, mode), 36);
    assert_eq!(size_of::<CreateDumb>(), 32);
    assert_eq!(offset_of!(CreateDumb, size), 24);
    assert_eq!(size_of::<MapDumb>(), 16);
    assert_eq!(size_of::<DestroyDumb>(), 4);
    assert_eq!(size_of::<FbCmd2>(), 104);
    assert_eq!(offset_of!(FbCmd2, handles), 20);
    assert_eq!(offset_of!(FbCmd2, modifier), 72);
    assert_eq!(size_of::<PageFlip>(), 24);
    assert_eq!(size_of::<GetCap>(), 16);
}

#[test]
fn the_ioctl_numbers_are_the_ones_libdrm_uses() {
    assert_eq!(IOCTL_SET_MASTER, 0x641E);
    assert_eq!(IOCTL_DROP_MASTER, 0x641F);
    assert_eq!(IOCTL_GET_CAP, 0xC010_640C);
    assert_eq!(IOCTL_GETRESOURCES, 0xC040_64A0);
    assert_eq!(IOCTL_GETCRTC, 0xC068_64A1);
    assert_eq!(IOCTL_SETCRTC, 0xC068_64A2);
    assert_eq!(IOCTL_GETENCODER, 0xC014_64A6);
    assert_eq!(IOCTL_GETCONNECTOR, 0xC050_64A7);
    assert_eq!(IOCTL_RMFB, 0xC004_64AF);
    assert_eq!(IOCTL_PAGE_FLIP, 0xC018_64B0);
    assert_eq!(IOCTL_CREATE_DUMB, 0xC020_64B2);
    assert_eq!(IOCTL_MAP_DUMB, 0xC010_64B3);
    assert_eq!(IOCTL_DESTROY_DUMB, 0xC004_64B4);
    assert_eq!(IOCTL_ADDFB2, 0xC068_64B8);
    assert_eq!(FORMAT_XRGB8888, 0x3432_5258);
}

#[test]
fn connector_names_match_the_sysfs_names() {
    assert_eq!(connector_name(14, 1), "eDP-1");
    assert_eq!(connector_name(11, 1), "HDMI-A-1");
    assert_eq!(connector_name(10, 2), "DP-2");
    assert_eq!(connector_name(7, 1), "LVDS-1");
    assert_eq!(connector_name(99, 3), "Unknown-3");
}

#[test]
fn the_real_backend_is_never_opened_in_tests_or_when_windowed() {
    assert!(RealDrm::open("/dev/dri/card0", false).is_err());
    assert!(RealDrm::open("/dev/dri/card0", true).is_err());
    assert_eq!(refuse_reason(false), Some("tests never open /dev/dri"));
    assert!(RealDrm::open("/nonexistent", false).err().unwrap().contains("tests never"));
}

#[test]
fn the_fake_card_hands_out_buffers_shows_them_and_records_it() {
    let c1 = FakeDrm::conn(10, "eDP-1", true, &[(2560, 1600, 60), (1920, 1200, 60)], 0b11);
    let mut d = FakeDrm::new(2, vec![c1]);
    let r = d.resources().unwrap();
    assert_eq!(r.crtcs.len(), 2);
    let c = d.connector(10).unwrap();
    assert!(c.modes[0].preferred() && !c.modes[1].preferred());
    assert_eq!(c.modes[0].size(), (2560, 1600));
    let b = d.create_buffer(2560, 1600).unwrap();
    assert!(b.pitch >= 2560 * 4 && b.size == b.pitch as u64 * 1600);
    d.pixels(&b).unwrap()[0] = 7;
    assert_eq!(d.pixels(&b).unwrap()[0], 7);
    d.set_crtc(r.crtcs[0], b.fb_id, &[10], Some(&c.modes[0])).unwrap();
    assert_eq!(d.shown[&r.crtcs[0]].2, (2560, 1600));
    d.page_flip(r.crtcs[0], b.fb_id).unwrap();
    assert_eq!(d.flips, 1);
    // a buffer smaller than the mode is refused, a failure can be made to happen once
    let small = d.create_buffer(640, 480).unwrap();
    assert!(d.set_crtc(r.crtcs[1], small.fb_id, &[10], Some(&c.modes[0])).is_err());
    d.fail = Some("create_buffer");
    assert!(d.create_buffer(10, 10).is_err());
    assert!(d.create_buffer(10, 10).is_ok());
    d.set_crtc(r.crtcs[0], 0, &[], None).unwrap();
    assert!(d.shown.is_empty());
    d.destroy_buffer(b);
    assert!(d.pixels(&b).is_err());
}

fn timing(clock: u32, htotal: u16, vtotal: u16, vrefresh: u32) -> ModeInfo {
    ModeInfo { clock, htotal, vtotal, vrefresh, hdisplay: 2560, vdisplay: 1600, ..Default::default() }
}

#[test]
fn a_mode_whose_vrefresh_is_zero_gets_its_rate_from_the_timing() {
    // 2560x1600 laptop panels as amdgpu reports them: vrefresh 0, the rate only in the timing
    assert_eq!(timing(345_000, 2720, 1646, 0).refresh_hz(), Some(77));
    assert_eq!(timing(268_000, 2720, 1646, 0).refresh_hz(), Some(60));
    assert_eq!(timing(738_800, 2720, 1646, 0).refresh_hz(), Some(165));
    assert_eq!(timing(1_070_000, 2720, 1646, 0).refresh_hz(), Some(239));
    // the kernel's own number wins when it is there
    assert_eq!(timing(268_000, 2720, 1646, 59).refresh_hz(), Some(59));
    // the classic 1080p60 timing (148.5 MHz, 2200 x 1125) is 60
    assert_eq!(timing(148_500, 2200, 1125, 0).refresh_hz(), Some(60));
}

#[test]
fn flags_change_how_the_rate_is_worked_out_and_an_unknown_rate_is_none() {
    // 1080i: 74.25 MHz over a 2200 x 1125 frame is 30 frames, which the kernel reports as 60 (fields) per second
    let mut i = timing(74_250, 2200, 1125, 0);
    i.flags = MODE_FLAG_INTERLACE;
    assert_eq!(i.refresh_hz(), Some(60));
    let mut d = timing(25_175, 800, 525, 0); // 640x480 at 60 Hz
    assert_eq!(d.refresh_hz(), Some(60));
    d.flags = MODE_FLAG_DBLSCAN; // double scan: each line twice, half the rate
    assert_eq!(d.refresh_hz(), Some(30));
    assert_eq!(timing(0, 2720, 1646, 0).refresh_hz(), None, "no clock");
    assert_eq!(timing(345_000, 0, 1646, 0).refresh_hz(), None, "no htotal");
    assert_eq!(timing(345_000, 2720, 0, 0).refresh_hz(), None, "no vtotal");
    assert_eq!(ModeInfo::default().refresh_hz(), None);
}
