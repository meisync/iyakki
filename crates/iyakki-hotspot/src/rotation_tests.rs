use super::*;

#[test]
fn nothing_is_due_at_first_and_the_timer_fires_after_the_interval_while_somebody_is_connected() {
    let r = Rotation::new(RotationPolicy::default(), 1000);
    assert_eq!(r.due(1000, 2), None);
    assert_eq!(r.due(4599, 2), None);
    assert_eq!(r.due(4600, 2), Some(Reason::Timer));
    assert_eq!(r.due(9999, 0), None, "nobody connected: nothing to protect");
    assert!(Reason::Timer.text().contains("set time") && Reason::StationLeft.text().contains("left"));
}

#[test]
fn a_station_leaving_asks_for_a_change_after_the_least_gap_and_one_change_serves_several_leavers() {
    let mut r = Rotation::new(RotationPolicy::default(), 0);
    r.station_left();
    assert_eq!(r.due(30, 1), None, "too soon after the last change");
    assert_eq!(r.due(60, 1), Some(Reason::StationLeft));
    r.station_left();
    r.station_left();
    r.done(61);
    assert_eq!(r.due(200, 1), None);
    assert_eq!(r.due(5000, 1), Some(Reason::Timer));
    // the last station left: nobody is connected, so no change is made now; it is asked for again when one joins
    let mut r = Rotation::new(RotationPolicy::default(), 0);
    r.station_left();
    assert_eq!(r.due(500, 0), None);
    assert_eq!(r.due(500, 1), Some(Reason::StationLeft));
}

#[test]
fn the_policy_can_turn_either_reason_off() {
    let mut r = Rotation::new(RotationPolicy { interval_secs: 0, on_leave: true, min_gap_secs: 60 }, 0);
    assert_eq!(r.due(1_000_000, 3), None, "no timer");
    r.station_left();
    assert_eq!(r.due(1_000_000, 3), Some(Reason::StationLeft));
    let mut r = Rotation::new(RotationPolicy { interval_secs: 600, on_leave: false, min_gap_secs: 60 }, 0);
    r.station_left();
    assert_eq!(r.due(100, 3), None, "leaving does not matter");
    assert_eq!(r.due(600, 3), Some(Reason::Timer));
}

#[test]
fn the_limits_of_a_policy_are_checked_in_plain_words() {
    assert!(RotationPolicy::default().checked().is_ok());
    assert!(RotationPolicy { interval_secs: 0, ..Default::default() }.checked().is_ok());
    assert!(RotationPolicy { interval_secs: 100, ..Default::default() }.checked().unwrap_err().contains("5 minutes"));
    assert!(RotationPolicy { min_gap_secs: 1, ..Default::default() }.checked().unwrap_err().contains("10 seconds"));
    assert!(RotationPolicy { min_gap_secs: 4000, ..Default::default() }.checked().is_err());
}

#[test]
fn a_loop_that_sleeps_is_told_when_to_look_again() {
    let mut r = Rotation::new(RotationPolicy::default(), 100);
    assert_eq!(r.next_check(100), 3700);
    r.station_left();
    assert_eq!(r.next_check(110), 160, "after the gap");
    assert_eq!(r.next_check(500), 500, "already due: now");
    r.done(500);
    assert_eq!(r.next_check(501), 4100);
    let off = Rotation::new(RotationPolicy { interval_secs: 0, on_leave: false, min_gap_secs: 60 }, 0);
    assert_eq!(off.next_check(10), 70);
}

#[test]
fn the_loop_step_rotates_with_the_next_index_and_a_refusal_waits_the_least_gap() {
    assert_eq!((next_key_id(1), next_key_id(2), next_key_id(3), next_key_id(0)), (2, 3, 1, 1));
    let mut r = Rotation::new(RotationPolicy::default(), 0);
    let mut log = Vec::new();
    let mut asked = Vec::new();
    assert_eq!(step(&mut r, 10, 1, 1, &mut |id| { asked.push(id); Ok(()) }, &mut |l| log.push(l)), None, "nothing due yet");
    r.station_left();
    assert_eq!(step(&mut r, 70, 1, 1, &mut |id| { asked.push(id); Err("busy".into()) }, &mut |l| log.push(l)), None);
    assert_eq!(asked, vec![2]);
    assert!(log[0].contains("not changed") && log[0].contains("busy"));
    assert_eq!(step(&mut r, 100, 1, 1, &mut |id| { asked.push(id); Ok(()) }, &mut |l| log.push(l)), None, "inside the least gap after the refusal");
    assert_eq!(step(&mut r, 130, 1, 1, &mut |id| { asked.push(id); Ok(()) }, &mut |l| log.push(l)), Some(Reason::StationLeft));
    assert_eq!(asked, vec![2, 2]);
    assert_eq!(step(&mut r, 131, 1, 2, &mut |_| Ok(()), &mut |_| {}), None, "done: the leaver is served");
}
