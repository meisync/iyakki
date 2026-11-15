use super::*;


fn vend(label_resp: u8, code: u8, pdu: u8, params: &[u8]) -> Vec<u8> {
    let mut v = vec![label_resp, 0x11, 0x0e, code, 0x48, 0x00, 0x00, 0x19, 0x58, pdu, 0, 0, params.len() as u8];
    v.extend_from_slice(params);
    v
}

#[test]
fn a_key_stroke_is_a_press_and_a_release_with_their_own_labels() {
    let mut c = Controller::new();
    let [down, up] = c.stroke(Key::Play);
    assert_eq!(down, vec![0x00, 0x11, 0x0e, 0x00, 0x48, 0x7c, 0x44, 0x00]);
    assert_eq!(up, vec![0x10, 0x11, 0x0e, 0x00, 0x48, 0x7c, 0xc4, 0x00]);
    // the phone accepts the press (response bit set in the first byte)
    let ok = [0x02, 0x11, 0x0e, 0x09, 0x48, 0x7c, 0x44, 0x00];
    assert_eq!(c.read(&ok), Ok(Answer::KeyAccepted(Key::Play, true)));
    let no = [0x12, 0x11, 0x0e, 0x08, 0x48, 0x7c, 0xc4, 0x00];
    assert_eq!(c.read(&no), Ok(Answer::KeyRefused(Key::Play)));
    // an answer with a label nobody used is not believed
    assert!(c.read(&ok).is_err());
}

#[test]
fn the_key_table_has_the_standard_ids_and_names_round_trip() {
    assert_eq!([Key::Pause.id(), Key::Stop.id(), Key::Next.id(), Key::Previous.id(), Key::VolumeUp.id(), Key::VolumeDown.id()], [0x46, 0x45, 0x4b, 0x4c, 0x41, 0x42]);
    for k in [Key::Play, Key::Pause, Key::Stop, Key::Next, Key::Previous, Key::Rewind, Key::FastForward, Key::VolumeUp, Key::VolumeDown, Key::Mute] {
        assert_eq!(Key::from_id(k.id()), Some(k));
        assert_eq!(Key::from_name(&k.name().to_uppercase()), Some(k));
    }
    assert_eq!(Key::from_id(0x00), None);
    assert_eq!(Key::from_name("explode"), None);
}

#[test]
fn the_controller_asks_for_events_status_notifications_and_volume_and_reads_the_answers() {
    let mut c = Controller::new();
    assert_eq!(c.ask_events(), vend(0x00, CTYPE_STATUS, PDU_GET_CAPABILITIES, &[0x03]));
    assert_eq!(c.read(&vend(0x02, RESP_STABLE, PDU_GET_CAPABILITIES, &[0x03, 0x02, 0x01, 0x0d])), Ok(Answer::Events(vec![1, 0x0d])));
    assert_eq!(c.ask_play_status(), vend(0x10, CTYPE_STATUS, PDU_GET_PLAY_STATUS, &[]));
    let st = [0x00, 0x03, 0x0d, 0x40, 0x00, 0x00, 0x03, 0xe8, 0x01];
    assert_eq!(c.read(&vend(0x12, RESP_STABLE, PDU_GET_PLAY_STATUS, &st)), Ok(Answer::PlayStatus { status: PlayStatus::Playing, length_ms: 200_000, position_ms: 1000 }));
    // a notification: the first answer is interim, the label stays open for the change
    assert_eq!(c.watch(EVENT_PLAYBACK_STATUS), vend(0x20, CTYPE_NOTIFY, PDU_REGISTER_NOTIFICATION, &[1, 0, 0, 0, 0]));
    assert_eq!(c.read(&vend(0x22, RESP_INTERIM, PDU_REGISTER_NOTIFICATION, &[1, 1])), Ok(Answer::Interim { event: 1, value: vec![1] }));
    assert_eq!(c.read(&vend(0x22, RESP_CHANGED, PDU_REGISTER_NOTIFICATION, &[1, 2])), Ok(Answer::Changed { event: 1, value: vec![2] }));
    // volume is cut to 127 and the answer is masked to 7 bits
    assert_eq!(c.set_volume(200), vend(0x30, CTYPE_CONTROL, PDU_SET_ABSOLUTE_VOLUME, &[127]));
    assert_eq!(c.read(&vend(0x32, RESP_ACCEPTED, PDU_SET_ABSOLUTE_VOLUME, &[0xff])), Ok(Answer::Volume(127)));
    // a refusal carries the error code; a device without the command says so
    c.set_volume(1);
    assert_eq!(c.read(&vend(0x42, RESP_REJECTED, PDU_SET_ABSOLUTE_VOLUME, &[ERR_INVALID_PARAMETER])), Ok(Answer::Rejected(1)));
    c.ask_play_status();
    assert_eq!(c.read(&vend(0x52, RESP_NOT_IMPLEMENTED, PDU_GET_PLAY_STATUS, &[])), Ok(Answer::NotImplemented));
}

#[test]
fn the_phone_sets_the_volume_and_asks_to_be_told_when_it_changes_here() {
    let mut t = Target::new(64);
    let (r, ev) = t.handle(&vend(0x30, CTYPE_CONTROL, PDU_SET_ABSOLUTE_VOLUME, &[0x40])).unwrap();
    assert_eq!(r.unwrap(), vend(0x32, RESP_ACCEPTED, PDU_SET_ABSOLUTE_VOLUME, &[0x40]));
    assert_eq!(ev, vec![Event::Volume(0x40)]);
    // bit 7 is not part of the volume
    let (_, ev) = t.handle(&vend(0x30, CTYPE_CONTROL, PDU_SET_ABSOLUTE_VOLUME, &[0xc1])).unwrap();
    assert_eq!(ev, vec![Event::Volume(0x41)]);
    // nobody asked yet: no notification
    assert_eq!(t.local_volume(10), None);
    let (r, _) = t.handle(&vend(0x40, CTYPE_NOTIFY, PDU_REGISTER_NOTIFICATION, &[0x0d, 0, 0, 0, 0])).unwrap();
    assert_eq!(r.unwrap(), vend(0x42, RESP_INTERIM, PDU_REGISTER_NOTIFICATION, &[0x0d, 10]));
    assert_eq!(t.local_volume(0x20).unwrap(), vend(0x42, RESP_CHANGED, PDU_REGISTER_NOTIFICATION, &[0x0d, 0x20]));
    // one notification per request
    assert_eq!(t.local_volume(0x21), None);
    assert_eq!(t.volume(), 0x21);
}

#[test]
fn the_target_tells_what_it_is_and_which_events_it_knows() {
    let mut t = Target::default();
    let unit = [0x60, 0x11, 0x0e, 0x01, 0x48, 0x30, 0xff, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(t.handle(&unit).unwrap().0.unwrap(), vec![0x62, 0x11, 0x0e, 0x0c, 0x48, 0x30, 0x07, 0x48, 0x00, 0x19, 0x58]);
    let sub = [0x70, 0x11, 0x0e, 0x01, 0x48, 0x31, 0x07, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(t.handle(&sub).unwrap().0.unwrap(), vec![0x72, 0x11, 0x0e, 0x0c, 0x48, 0x31, 0x07, 0x48, 0xff, 0xff, 0xff]);
    let (r, _) = t.handle(&vend(0x50, CTYPE_STATUS, PDU_GET_CAPABILITIES, &[0x03])).unwrap();
    assert_eq!(r.unwrap(), vend(0x52, RESP_STABLE, PDU_GET_CAPABILITIES, &[0x03, 0x01, 0x0d]));
    let (r, _) = t.handle(&vend(0x50, CTYPE_STATUS, PDU_GET_CAPABILITIES, &[0x02])).unwrap();
    assert_eq!(r.unwrap(), vend(0x52, RESP_STABLE, PDU_GET_CAPABILITIES, &[0x02, 0x01, 0x00, 0x19, 0x58]));
    // bad capability id, an event we do not tell about, a vendor command we do not know
    let (r, _) = t.handle(&vend(0x50, CTYPE_STATUS, PDU_GET_CAPABILITIES, &[0x09])).unwrap();
    assert_eq!(r.unwrap(), vend(0x52, RESP_REJECTED, PDU_GET_CAPABILITIES, &[ERR_INVALID_PARAMETER]));
    let (r, _) = t.handle(&vend(0x50, CTYPE_NOTIFY, PDU_REGISTER_NOTIFICATION, &[0x02, 0, 0, 0, 0])).unwrap();
    assert_eq!(r.unwrap(), vend(0x52, RESP_REJECTED, PDU_REGISTER_NOTIFICATION, &[ERR_INVALID_PARAMETER]));
    let (r, _) = t.handle(&vend(0x50, CTYPE_CONTROL, 0x7f, &[1])).unwrap();
    assert_eq!(r.unwrap(), vend(0x52, RESP_NOT_IMPLEMENTED, 0x7f, &[1]));
}

#[test]
fn volume_keys_from_the_phone_side_are_events_and_other_keys_are_not_taken() {
    let mut t = Target::default();
    let (r, ev) = t.handle(&[0x00, 0x11, 0x0e, 0x00, 0x48, 0x7c, 0x41, 0x00]).unwrap();
    assert_eq!(r.unwrap(), vec![0x02, 0x11, 0x0e, 0x09, 0x48, 0x7c, 0x41, 0x00]);
    assert_eq!(ev, vec![Event::Key(Key::VolumeUp, true)]);
    let (r, ev) = t.handle(&[0x10, 0x11, 0x0e, 0x00, 0x48, 0x7c, 0xc1, 0x00]).unwrap();
    assert_eq!((r.unwrap()[3], ev), (RESP_ACCEPTED, vec![Event::Key(Key::VolumeUp, false)]));
    // play, pause and stop are taken (and become the player's)
    let (r, ev) = t.handle(&[0x20, 0x11, 0x0e, 0x00, 0x48, 0x7c, 0x44, 0x00]).unwrap();
    assert_eq!((r.unwrap()[3], ev.clone()), (RESP_ACCEPTED, vec![Event::Key(Key::Play, true)]));
    assert_eq!(act_for(&ev[0]), Some(Act::Media(MediaKey::Play)));
    assert_eq!(act_for(&Event::Key(Key::Pause, true)), Some(Act::Media(MediaKey::Pause)));
    assert_eq!(act_for(&Event::Key(Key::Stop, true)), Some(Act::Media(MediaKey::Stop)));
    assert_eq!(act_for(&Event::Key(Key::Play, false)), None, "the release does nothing");
    // next and previous are taken too: they step the play queue
    for (id, k) in [(0x4b, Key::Next), (0x4c, Key::Previous)] {
        let (r, ev) = t.handle(&[0x20, 0x11, 0x0e, 0x00, 0x48, 0x7c, id, 0x00]).unwrap();
        assert_eq!((r.unwrap()[3], ev), (RESP_ACCEPTED, vec![Event::Key(k, true)]), "key {id:#x}");
    }
    // rewind and fast forward are not
    for id in [0x48, 0x49] {
        let (r, ev) = t.handle(&[0x20, 0x11, 0x0e, 0x00, 0x48, 0x7c, id, 0x00]).unwrap();
        assert_eq!((r.unwrap()[3], ev), (RESP_NOT_IMPLEMENTED, vec![]), "key {id:#x}");
    }
    let (r, _) = t.handle(&[0x30, 0x11, 0x0e, 0x00, 0x48, 0x7c, 0x77, 0x00]).unwrap();
    assert_eq!(r.unwrap()[3], RESP_NOT_IMPLEMENTED);
    // an unknown opcode
    assert_eq!(t.handle(&[0x40, 0x11, 0x0e, 0x00, 0x48, 0x20]).unwrap().0.unwrap()[3], RESP_NOT_IMPLEMENTED);
}

#[test]
fn another_profile_gets_the_invalid_profile_answer_and_pieces_and_cut_packets_are_errors() {
    let mut t = Target::default();
    assert_eq!(t.handle(&[0x70, 0x11, 0x0f, 0x00, 0x48, 0x00]).unwrap().0.unwrap(), vec![0x73, 0x11, 0x0f]);
    // start of a fragmented message
    assert_eq!(t.handle(&[0x04, 0x02, 0x11, 0x0e]), Err(AvrcpError::Fragmented));
    assert_eq!(t.handle(&[0x00, 0x11]), Err(AvrcpError::Short));
    assert_eq!(t.handle(&[0x00, 0x11, 0x0e, 0x00, 0x48]), Err(AvrcpError::Short));
    // the other side says it does not know AVRCP
    assert_eq!(t.handle(&[0x03, 0x11, 0x0e]), Err(AvrcpError::WrongProfile));
    // answers from the phone need no answer
    assert_eq!(t.handle(&[0x02, 0x11, 0x0e, 0x09, 0x48, 0x7c, 0x44, 0x00]).unwrap(), (None, vec![]));
    for e in [AvrcpError::Short, AvrcpError::Fragmented, AvrcpError::WrongProfile, AvrcpError::OtherProfile(1)] {
        assert!(!e.text().is_empty());
    }
}

#[test]
fn damaged_vendor_frames_are_refused_and_hostile_bytes_never_panic() {
    let mut t = Target::default();
    // length field larger than the data
    let bad = [0x50, 0x11, 0x0e, 0x00, 0x48, 0x00, 0x00, 0x19, 0x58, 0x50, 0x00, 0x00, 0x05, 0x40];
    assert_eq!(t.handle(&bad).unwrap().0.unwrap()[3], RESP_REJECTED);
    // another company id
    let other = [0x50, 0x11, 0x0e, 0x00, 0x48, 0x00, 0x00, 0x00, 0x01, 0x50, 0x00, 0x00, 0x01, 0x40];
    assert_eq!(t.handle(&other).unwrap().0.unwrap()[3], RESP_REJECTED);
    // set volume without its byte
    let (r, _) = t.handle(&vend(0x50, CTYPE_CONTROL, PDU_SET_ABSOLUTE_VOLUME, &[])).unwrap();
    assert_eq!(r.unwrap(), vend(0x52, RESP_REJECTED, PDU_SET_ABSOLUTE_VOLUME, &[ERR_INVALID_COMMAND]));
    let mut c = Controller::new();
    c.ask_events();
    let mut x: u32 = 7;
    for _ in 0..3000 {
        let n = (x % 24) as usize;
        let d: Vec<u8> = (0..n).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect();
        let _ = t.handle(&d);
        let _ = c.read(&d);
        x = x.wrapping_add(1);
    }
}

#[test]
fn volume_converts_between_percent_and_the_protocols_range() {
    assert_eq!((percent_volume(0), percent_volume(50), percent_volume(100), percent_volume(250)), (0, 64, 127, 127));
    assert_eq!((volume_percent(0), volume_percent(64), volume_percent(127), volume_percent(255)), (0, 50, 100, 100));
    for p in 0..=100 {
        assert_eq!(volume_percent(percent_volume(p)), p, "{p}");
    }
}

#[test]
fn what_the_phone_does_becomes_a_system_action_and_only_a_press_counts() {
    assert_eq!(act_for(&Event::Volume(127)), Some(Act::SetPercent(100)));
    assert_eq!(act_for(&Event::Volume(0)), Some(Act::SetPercent(0)));
    assert_eq!(act_for(&Event::Key(Key::VolumeUp, true)), Some(Act::Step(KEY_STEP_PERCENT)));
    assert_eq!(act_for(&Event::Key(Key::VolumeDown, true)), Some(Act::Step(-KEY_STEP_PERCENT)));
    assert_eq!(act_for(&Event::Key(Key::Mute, true)), Some(Act::ToggleMute));
    assert_eq!(act_for(&Event::Key(Key::VolumeUp, false)), None);
    assert_eq!(act_for(&Event::Key(Key::Next, true)), Some(Act::Media(MediaKey::Next)));
    assert_eq!(act_for(&Event::Key(Key::Previous, true)), Some(Act::Media(MediaKey::Previous)));
    assert_eq!(act_for(&Event::Key(Key::Rewind, true)), None, "rewind and fast forward are still not taken");
    assert_eq!(act_for(&Event::Key(Key::Next, false)), None, "only a press counts");
}
