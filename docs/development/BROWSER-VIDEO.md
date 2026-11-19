# Browser video in the full engine: "Something went wrong" after a minute (found 2026-11-16)

## What the owner saw
YouTube's Big Buck Bunny 60 fps (and a playlist on a Mac mini) showed "Something went wrong. Refresh or try again later." after a
minute or several minutes. Short clips and a 3.5 minute 1080p video played. The sound sink logged an underrun and a stop and start.

## What it is not
Measured on the Lenovo (Ryzen 7 7745HX, software VP9 through libvpx, Opus, our own web sink), with a probe in the page
(`dist/webview/wpe-helper/media-probe.js`, lines starting `page:` in the system log while `/tmp/iyakki-dev/gst-debug` exists):
* Decoding keeps up: 1920x1080 at 60 fps, 0 dropped frames in 4000+ frames, no GStreamer error, no crash, the web process stays alive.
* The sink stop and start and the one underrun are the player tearing its video element down (`emptied`), not a cause.
* The page does not get a media error (`video.error` stays empty): the player itself gives up.

## What it is
YouTube streams in short answers (server-driven adaptive streaming, "SABR", `POST ...videoplayback?sabr=1`). Every answer carries a
protection status. Measured on the failing page loads: status 2 ("still checking") in the first answers, then status 3 ("required")
and answers of 123 bytes with no media, from video second 60 on; the buffer stops at 60.0 s and the player shows the error 20-40 s
later. On the good page loads the same answers say status 1 and the buffer grows to the end of the video. The only difference is
whether YouTube accepts the "proof of origin" token that a program of YouTube's (`/api/jnn/v1/GenerateIT`) makes inside the page. Same
image, same video, same minute: about 6 page loads in 10 were accepted, 4 in 10 were not (28 loads counted, 2026-11-16).

Things tried that did not change the share (a screen value instead of 0x0, a plain Safari user agent) are kept: the screen value is
correct anyway (`page-env.js`); the user agent is unchanged.

## What was done
* `video-recover.js`: when the YouTube player shows its error, the page is loaded again at the second the video had reached (what the
  message asks for; a new page makes a new token), up to 6 times in 10 minutes for one tab, and the player's play button is pressed once
  by a real click through the helper (`playclick X Y` message; a script's own click does not start a YouTube video, and starting the
  video again inside the same page, `loadVideoById`, got the same refusal at once, both measured). The helper passes the click on only
  for a youtube.com page, inside the page area, at most once per 5 s and 8 per 10 minutes. `IYAKKI_WEBVIEW_VIDEO_RECOVER=0` turns the
  recovery off.
* `page-env.js`: `screen`, `outerWidth/Height` answer the page area instead of 0 x 0 when the engine reports none.
* `media-probe.js` diagnosis lines (only with the developer switch file `/tmp/iyakki-dev/gst-debug`) so the next failure says why:
  `iyakki dev logs --system | grep "page:"`.

## Not solved here
The share of tokens YouTube refuses is YouTube's decision about the page's environment; the engine cannot make a token itself.
Hardware decoding (VA-API) is not in the Lenovo image: the AMD card needs Mesa's video drivers and the `va` plugin, which the
`WEB_VAAPI` fragment does not yet cover (Intel only). 4K60 in software was not needed to get a stable picture: YouTube chose 1080p60.
