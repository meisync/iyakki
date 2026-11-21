# IYAKKI web view helper (WPE WebKit). Plain make; Buildroot passes CC / PKG_CONFIG / CFLAGS / LDFLAGS.
#   make            builds ./webview-helper
#   make install DESTDIR=... installs /usr/libexec/iyakki/webview-helper
CC ?= cc
PKG_CONFIG ?= pkg-config
PKGS = wpe-webkit-2.0 wpebackend-fdo-1.0 wayland-server egl glesv2 gbm glib-2.0 gio-2.0 alsa
CFLAGS ?= -O2
override CFLAGS += -std=gnu11 -Wall -Wextra -Wno-unused-parameter -Wno-array-bounds $(shell $(PKG_CONFIG) --cflags $(PKGS))
LDLIBS += $(shell $(PKG_CONFIG) --libs $(PKGS))

# media-probe.js becomes a C string (media-probe.h) so the helper needs no data file at run time
media-probe.h: media-probe.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' media-probe.js > $@

# dev-probe.js (developer tools for the chat agent) the same way
dev-probe.h: dev-probe.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' dev-probe.js > $@

# page-env.js (screen and window values for the offscreen page) the same way
page-env.h: page-env.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' page-env.js > $@

# site-probe.js (key system and manifest report for site apps) the same way
site-probe.h: site-probe.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' site-probe.js > $@

# video-recover.js (a YouTube player that shows its error is loaded again) the same way
video-recover.h: video-recover.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' video-recover.js > $@

# media-cap.js (the video ceiling and the decoder switch the person or the tenant chose) the same way
media-cap.h: media-cap.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' media-cap.js > $@

webview-helper: helper.c proto.h extsnap.h mediapolicy.h media-cap.h micscan.h media-probe.h dev-probe.h page-env.h video-recover.h site-probe.h
	$(CC) $(CFLAGS) $(LDFLAGS) -Wl,--strip-debug -o $@ helper.c $(LDLIBS)

# The shared camera: a GStreamer plugin the web process loads (camshare.c). The engine opens one source per getUserMedia call and a
# USB camera cannot be opened twice; the plugin makes the engine see every camera through one open.
GSTPKGS = gstreamer-1.0 gstreamer-app-1.0
GSTCFLAGS = $(shell $(PKG_CONFIG) --cflags $(GSTPKGS))
GSTLIBS = $(shell $(PKG_CONFIG) --libs $(GSTPKGS))
libgstiyakkicamshare.so: camshare.c devlist.h
	$(CC) $(CFLAGS) $(GSTCFLAGS) $(LDFLAGS) -shared -fPIC -Wl,--strip-debug -o $@ camshare.c $(GSTLIBS) -lm

# The microphones: a GStreamer device provider plugin (micshare.c, scan logic in micscan.h). Capture streams are listed from /proc/asound,
# opened through plughw (rate, format and channels converted by alsa-lib) and shared between page requests.
MICPKGS = gstreamer-1.0 gstreamer-app-1.0 alsa
MICCFLAGS = $(shell $(PKG_CONFIG) --cflags $(MICPKGS))
MICLIBS = $(shell $(PKG_CONFIG) --libs $(MICPKGS))
libgstiyakkimic.so: micshare.c micscan.h devlist.h
	$(CC) $(CFLAGS) $(MICCFLAGS) $(LDFLAGS) -shared -fPIC -Wl,--strip-debug -o $@ micshare.c $(MICLIBS)

# The MP3 decoder: a GStreamer audio decoder element (mp3dec.c) around minimp3 (third_party/minimp3.h, CC0). The image has no other MP3 decoder.
MP3PKGS = gstreamer-1.0 gstreamer-audio-1.0
MP3CFLAGS = $(shell $(PKG_CONFIG) --cflags $(MP3PKGS))
MP3LIBS = $(shell $(PKG_CONFIG) --libs $(MP3PKGS))
libgstiyakkimp3.so: mp3dec.c third_party/minimp3.h
	$(CC) $(CFLAGS) $(MP3CFLAGS) $(LDFLAGS) -shared -fPIC -Wl,--strip-debug -o $@ mp3dec.c $(MP3LIBS)

# The engine's sound output that cannot fail (websink.c): ALSA the way aplay asks, silence when the device cannot be used.
WEBSINKPKGS = gstreamer-1.0 gstreamer-audio-1.0 alsa
WEBSINKCFLAGS = $(shell $(PKG_CONFIG) --cflags $(WEBSINKPKGS))
WEBSINKLIBS = $(shell $(PKG_CONFIG) --libs $(WEBSINKPKGS))
libgstiyakkiwebsink.so: websink.c
	$(CC) $(CFLAGS) $(WEBSINKCFLAGS) $(LDFLAGS) -shared -fPIC -Wl,--strip-debug -o $@ websink.c $(WEBSINKLIBS)

# The web process extension (webext.c, parsing and matching in extsnap.h): custom request headers and later browser extensions, loaded
# into every WPE web process from /usr/lib/iyakki/webext. The extension API package is named wpe-web-extension-2.0 in new WPE
# WebKit and wpe-web-process-extension-2.0 in older ones; the first that exists is used. Not yet built against a real WPE.
WEBEXTPKG = $(shell $(PKG_CONFIG) --exists wpe-web-extension-2.0 && echo wpe-web-extension-2.0 || echo wpe-web-process-extension-2.0)
WEBEXTPKGS = $(WEBEXTPKG) wpe-webkit-2.0 libsoup-3.0 glib-2.0 gmodule-2.0
WEBEXTCFLAGS = $(shell $(PKG_CONFIG) --cflags $(WEBEXTPKGS))
WEBEXTLIBS = $(shell $(PKG_CONFIG) --libs $(WEBEXTPKGS))
libiyakkiwebext.so: webext.c extsnap.h
	$(CC) $(CFLAGS) $(WEBEXTCFLAGS) $(LDFLAGS) -shared -fPIC -Wl,--strip-debug -o $@ webext.c $(WEBEXTLIBS)

# The screen portal (screenportal.c, feed reader in screenfeed.h, names in portalpure.h): a program the helper starts when IYAKKI_WEBVIEW_SCREEN=1.
# It needs dbus-daemon and pipewire (with its GStreamer plug-in) at run time. Not run in this tree.
PORTALPKGS = gio-unix-2.0 glib-2.0 gstreamer-1.0 gstreamer-app-1.0
PORTALCFLAGS = $(shell $(PKG_CONFIG) --cflags $(PORTALPKGS))
PORTALLIBS = $(shell $(PKG_CONFIG) --libs $(PORTALPKGS))
iyakki-screenportal: screenportal.c portalpure.h screenfeed.h
	$(CC) $(CFLAGS) $(PORTALCFLAGS) $(LDFLAGS) -Wl,--strip-debug -o $@ screenportal.c $(PORTALLIBS)

mic-check: mic-check.c micshare.c micscan.h devlist.h
	$(CC) $(CFLAGS) $(MICCFLAGS) $(LDFLAGS) -o $@ mic-check.c $(MICLIBS)

# `make check`: the mode choice and the sharing, against a test camera made of videotestsrc (needs GStreamer, no camera)
camshare-check: camshare-check.c camshare.c devlist.h
	$(CC) $(CFLAGS) $(GSTCFLAGS) $(LDFLAGS) -o $@ camshare-check.c $(GSTLIBS) -lm
# websink-check: the web sound sink's ts-offset property, its log lines and audio through it (null ALSA device, no sound card)
websink-check: websink-check.c websink.c
	$(CC) $(CFLAGS) $(WEBSINKCFLAGS) $(LDFLAGS) -o $@ websink-check.c $(WEBSINKLIBS)
# ext-check: extsnap.h against a snapshot and cases written by the Rust side (run by cargo test -p iyakki-agent, browserext)
ext-check: ext-check.c extsnap.h
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ ext-check.c

check: camshare-check mic-check websink-check
	./camshare-check
	./mic-check
	./websink-check

install: webview-helper
	install -D -m 0755 webview-helper $(DESTDIR)/usr/libexec/iyakki/webview-helper

install-camshare: libgstiyakkicamshare.so
	install -D -m 0755 libgstiyakkicamshare.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so

install-websink: libgstiyakkiwebsink.so
	install -D -m 0755 libgstiyakkiwebsink.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so

install-mp3: libgstiyakkimp3.so
	install -D -m 0755 libgstiyakkimp3.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkimp3.so

install-portal: iyakki-screenportal
	install -D -m 0755 iyakki-screenportal $(DESTDIR)/usr/libexec/iyakki/iyakki-screenportal

install-webext: libiyakkiwebext.so
	install -D -m 0755 libiyakkiwebext.so $(DESTDIR)/usr/lib/iyakki/webext/libiyakkiwebext.so

install-mic: libgstiyakkimic.so
	install -D -m 0755 libgstiyakkimic.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkimic.so

clean:
	rm -f iyakki-screenportal libiyakkiwebext.so ext-check webview-helper media-probe.h dev-probe.h page-env.h video-recover.h libgstiyakkicamshare.so camshare-check websink-check libgstiyakkimic.so mic-check libgstiyakkimp3.so libgstiyakkiwebsink.so

# media-cap.h and the other generated headers are listed under `clean` with the rest
.PHONY: install-portal install-webext install install-camshare install-mic install-mp3 install-websink check clean
