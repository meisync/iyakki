# CHECK-UP

CHECK-UP is a guided hardware self-test. It walks through the parts of this computer one step at a time; every step ends as **Pass**,
**Fail** or **Skipped**, in plain words, with advice and (on a failure) the same fix button DOCTOR would offer. The result can be
saved through Share diagnostics. (Backlog P1; matrix rows F-CHECKUP-01 to F-CHECKUP-03, F-SELFTEST-01.)

## The tile

Devices & System group, for administrators, operators and users (not guests). Nothing runs by itself: you start each step.

| Key | Does |
| --- | --- |
| Up / Down | choose a step |
| Enter | start the chosen step (or answer yes while it waits) |
| `y` / `n` | yes / no to "did you see / hear it" |
| `k` | skip the step |
| `f` | the fix button of a failed step (opens the Settings page, like DOCTOR) |
| `d` | open DOCTOR |
| `r` | start over (the computer is read again) |
| `s` | Share result |
| Esc | stop what is running, then leave |

## The steps

| Step | What happens | Skipped when |
| --- | --- | --- |
| Screen | four colour bars in a frame; you say whether you see them all | you skip it |
| Touch screen | touch a circle | the computer lists no touch screen |
| Keyboard | press three different keys (every key counts while it listens) | you skip it |
| Mouse or touch pad | click a circle | you skip it |
| Speaker | a short two-note tone through the IYAKKI mixer (the one audio entry point); you say whether you heard it | you skip it |
| Microphone | records **2 seconds** from the microphone the Voice tile uses (`VoiceSettings.mic`, else the first listed) and shows a level meter | no real microphone is listed, or a T2 Mac's capture switch is off: the step says so |
| Camera | one picture from the camera the CAMERA tile would use, shown in the step | no camera is listed |
| External monitor | lists the connected outputs and their size | only the built-in screen is connected |
| Network | an adapter, a link and a way out | never (no adapter is a Fail) |
| Wi-Fi | the adapter is listed and not switched off | no Wi-Fi adapter |
| Bluetooth | the adapter is listed and not switched off | no Bluetooth adapter |
| Battery and power | charge, charger, wear; a laptop with no battery listed fails | not a laptop |

A failed step shows what it found, what to do, and a button that opens the Settings page (Display, Sound, Network, Bluetooth, Battery,
Keyboard, Browser settings for a busy camera), plus **Open DOCTOR**. The advice and the Settings page are the ones DOCTOR uses for the
same trouble (sound card missing, output not connected, microphone or camera trouble).

## Camera and microphone: consent

* Choosing a step, scrolling or opening the tile never opens the camera or the microphone. They start only on a click or Enter on
  their own button.
* While they work the shell shows the same sign as for a web page, **Microphone on** or **Camera on** (clickable to stop), and
  the step shows the same words with a blinking dot. The sign goes out when the step ends, is stopped (Stop, Esc, a click on the
  sign), the person moves to another step or the tile is left.
* **Nothing recorded is kept.** The 2 seconds of sound are measured for loudness and dropped; the camera picture is shown once and
  dropped when you move to another step. Neither is written to disk, and the report holds only verdicts and device names.
* Starting either step is audited (`checkup.microphone`, `checkup.camera`) without content.

## Share result

`s` (or the button) opens DOCTOR's Share diagnostics preview. The newest report is kept in memory and is the bundle's **Check-up**
section: a summary line (`3 passed, 1 failed, 2 skipped, 6 not done yet`), one line per step and the advice under each failure. You
read exactly what would be saved, then save it to your home folder or a USB stick yourself; it is redacted like every other section
and nothing is uploaded. Opening DOCTOR's own Share diagnostics afterwards includes the same section.

## Where the code is

* `crates/iyakki-agent/src/checkup.rs`: the steps' decisions, the readers (`/proc/bus/input/devices`, `/sys/class/net`, rfkill,
  Bluetooth, battery), the tone, the report.
* `crates/iyakki-shell/src/apps/checkup.rs`, `checkup_ui.rs`: the tile. `crates/iyakki-shell/src/camera/probe.rs`: the one camera
  picture.
* Tests: `checkup_tests.rs` in both crates, with fake recorders, a fake speaker, a fake camera and a fake sign.

## What is not verified: the manual list for a real computer

Everything above is tested with made-up machines. Only a real computer (the Mac, a PC) can confirm:

1. **Screen**: the four bars look right at the panel's real size; the frame is not cut off.
2. **Touch**: on a touch screen the circle answers a finger; on a computer without one the step is Skipped.
3. **Keyboard / pointer**: the built-in keyboard (every row, the Fn keys), a USB keyboard and a mouse or touch pad.
4. **Speaker**: the tone is heard on the built-in speaker, on headphones and over HDMI (Settings -> Sound output), at a safe volume, and
   Radio or a web page playing at the same time is mixed, not cut.
5. **Microphone**: a USB microphone shows a moving meter and a Pass; on a T2 Mac the step says the capture switch is off (and Skipped);
   after an administrator turns the switch on, the built-in microphones work; the sign "Microphone on" appears and goes out; the
   microphone chosen in the Voice tile is the one used.
6. **Camera**: a USB webcam gives a picture and a Pass; a covered lens fails with the lens-cover advice; a camera held by the Browser fails
   with the busy advice; "Camera on" shows and goes out; with no camera the step says why (FaceTime HD needs the firmware download).
7. **External monitor**: HDMI / DisplayPort / USB-C monitors are listed with their size; unplugging one and running the step again
   changes the answer.
8. **Network / Wi-Fi / Bluetooth**: a cable pulled out fails the network step; the airplane-mode key makes Wi-Fi and Bluetooth fail with
   the switch advice; Macs without firmware say Wi-Fi is missing.
9. **Battery**: a laptop shows charge, charger and wear; a desktop is Skipped; a laptop with a driverless battery fails with the
   driver advice.
10. **Share result**: after a run, DOCTOR's preview holds the Check-up section and no secret; saving it to the home folder and to a USB stick
    works.
