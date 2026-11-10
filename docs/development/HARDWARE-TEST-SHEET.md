# Hardware test sheet: what to try on your machines (image 1109b, 2026-11-09)

Written for the owner. Each line is one thing to do, what you should see, and a box for the result. "Auto" means the orchestrator already
checked it on the real machine (not a person at the keyboard). Report anything that differs with the text on the screen.

Machines: Mac mini 2018 (internal install and the T7), MacBook Pro 14,1, HP Pavilion 16, Lenovo Legion Pro 5. The T7 install moves between them
(`iyakki dev info` shows the machine: always look at the "device" line first).

## Already checked on real hardware (do not repeat unless it breaks)

| What | Where | By |
|---|---|---|
| Image 1109a boots, normal start, signed loader, no menu countdown | all four | you |
| Bluetooth: pair a phone, phone sound plays (A2DP), tether gets an IP and carries internet | MacBook Pro 14,1 | you |
| Bluetooth page redesign (switch card, paired list, scan popup with the nearby list, popup pairing) | Mac mini | you |
| Live USB camera view, red band, snapshot (valid 1280x720 JPEG) | Mac mini | auto |
| Domains: `store.local` to a cluster service, 200 through the listener, 404 for an unknown name | Mac mini | auto |
| PDF viewer opens, renders, searches | Mac mini | auto |
| Accessibility page, high contrast on and off | Mac mini | auto |
| Image 1109b: new kernel has device-mapper, dm-crypt, loop, XTS, AES-NI; boot confirmed; no panic | Mac mini | auto |

## 1. First, after restarting onto 1109b (5 minutes)

1. Sign in. Expect: the Home screen, no errors in the notification bell beyond the usual.
2. CAMERAS with two webcams plugged in. Expect: both "Live". The second one (HD 4MP WEBCAM) used to say "the camera refused 1280x720": it should now
   come up at a smaller picture size. If it still says Lost, copy the text.
3. Settings, search "domains". Expect: the Domains page is the first result (before VPN).

## 2. Encrypted home (new kernel needed: only on 1109b or later)

Done on the Mac mini 2018 on 2026-11-10 (image 1109i, scratch account): 2.1 (through Move), 2.2, 2.3, 2.4, 2.6 and 2.7 passed. Still to try: 2.5 (an administrator's password reset), Make on a fresh account, a busy home refusing to lock, and the other machines.

Use a scratch account, not your main one. Settings, Account and data, **Encrypted home**.

1. Make the home. Expect: a recovery key shown once (14 groups of 4). Type it back. Expect: "saved" only after it opens the volume.
2. Write a file in the home, sign out, sign in. Expect: the file is there.
3. Sign in with a wrong password three times. Expect: the same sentence each time and a growing wait (2, 5, 15 s), never which part failed.
4. Change the password. Expect: the home still opens with the new one; the recovery key still works.
5. As an administrator, reset that account's password. Expect: a warning that the home stays sealed. Open it with the recovery key.
6. Lock now. Expect: the files are gone from the folder until you open again; a busy folder says "in use", never forced.
7. Optional, Move existing files: it copies, verifies, switches, and keeps `<name>.plain-before-encrypt`. Check that copy is still there.

## 3. Bluetooth passkey and PIN entry (needs a phone or device that shows a number)

1. Settings, Bluetooth, power on, Scan, Pair the device. If it shows a 6-digit number to type here, expect: a popup with six boxes.
2. Type it. Expect: "Paired". Type a wrong length. Expect: a plain refusal and the popup stays open.
3. Cancel with Esc. Expect: the device is told no, nothing is stored.

## 4. Lock screen (you type your password, so I could not test it)

1. Press Super+L (a PC keyboard), Control+Command+Q or Control+Command+L (a Mac keyboard in the Mac profile: Command+L is the browser's), or choose Lock in the Power menu. Expect: a lock card with one password field; apps and runs keep going behind it.
2. Type a wrong password. Expect: a refusal and a wait. Type yours. Expect: back to where you were.
3. Settings, Display and sound, Screen saver: set "Lock the screen when idle" to 5 minutes, wait. Expect: the screen locks (the default is Never).

## 5. Cameras, recording and motion (hard asks: you press Allow)

1. CAMERAS, select the USB camera, press `p`, then `r`. Expect: a confirmation card. Allow. Expect: a red REC sign on every screen.
2. Wait 70 seconds, open the recordings timeline. Expect: a clip you can play and step. Click the red sign. Expect: recording stops.
3. Look at the files under the data folder `cameras/recordings`: they must not contain readable pictures (they are encrypted).
4. Network camera (RTSP or HTTP): `a`, enter the address and a view-only user. Expect: Connecting, then Live. Then `m`, then `w`: watching on,
   a blue MOTION pill; walk in front of it. Expect: an event with a thumbnail in the list and an inbox notice (if you switched alerts on).
5. Draw a mask over part of the picture and tick "also in recordings and snapshots". Expect: that area is black in a snapshot.

## 6. Files: SFTP server and REMOTE FILES

1. Ask the assistant: "turn on the SFTP server for my account with this key" (give a public key). Approve the cards. Expect: port 2222, a host key fingerprint.
2. From another computer: `sftp -P 2222 ACCOUNT@ADDRESS`. Compare the fingerprint. Try `ls`, `put`, `get`, `mkdir`; then `cd ..`, `ls /etc`.
   Expect: nothing outside your folders appears.
3. REMOTE FILES tile: add a Linux machine over SFTP. Expect: a fingerprint card (press y after comparing), then the folder list; upload and
   download a file; swap the server's host key and reconnect. Expect: a red refusal with no accept button.

## 7. Voice, accessibility and the rest

1. Voice tile, ASSISTANT tab (F5): turn it on, Talk, say "what time is it". Expect: an answer aloud, a red dot only while listening. Needs a USB microphone,
   a reading voice and a chat model. Say "set the follow-up window to 5 seconds": the card needs a tap or key, a spoken yes is refused.
2. Settings, Accessibility: Super+Alt+S with a reading voice installed. Expect: "Screen reader on", focus changes spoken, a password field says
   only "5 characters typed".
3. Browser tile, a normal page, press V then H: browse mode jumps between headings; K opens the links list.
4. Wake phrase: record the takes (see WAKE-PHRASE.md), train, evaluate. Nothing listens yet.
5. A phone playing music through the MacBook with the designed SBC window (SBC-TABLES.md section 7): same song, old build and new build, which
   sounds better? The default is still the old window until you say the new one is fine.

## 8. Report format

For each step: pass, fail with the exact text on screen, or "could not try". The most useful failures to report first: step 1.2 (second webcam),
section 2 (encrypted home), section 5 (recording and motion).
