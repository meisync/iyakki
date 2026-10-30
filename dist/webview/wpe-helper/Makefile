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

webview-helper: helper.c proto.h micscan.h media-probe.h dev-probe.h
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

mic-check: mic-check.c micshare.c micscan.h devlist.h
	$(CC) $(CFLAGS) $(MICCFLAGS) $(LDFLAGS) -o $@ mic-check.c $(MICLIBS)

# `make check`: the mode choice and the sharing, against a test camera made of videotestsrc (needs GStreamer, no camera)
camshare-check: camshare-check.c camshare.c devlist.h
	$(CC) $(CFLAGS) $(GSTCFLAGS) $(LDFLAGS) -o $@ camshare-check.c $(GSTLIBS) -lm
check: camshare-check mic-check
	./camshare-check
	./mic-check

install: webview-helper
	install -D -m 0755 webview-helper $(DESTDIR)/usr/libexec/iyakki/webview-helper

install-camshare: libgstiyakkicamshare.so
	install -D -m 0755 libgstiyakkicamshare.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so

install-websink: libgstiyakkiwebsink.so
	install -D -m 0755 libgstiyakkiwebsink.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so

install-mp3: libgstiyakkimp3.so
	install -D -m 0755 libgstiyakkimp3.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkimp3.so

install-mic: libgstiyakkimic.so
	install -D -m 0755 libgstiyakkimic.so $(DESTDIR)/usr/lib/gstreamer-1.0/libgstiyakkimic.so

clean:
	rm -f webview-helper media-probe.h dev-probe.h libgstiyakkicamshare.so camshare-check libgstiyakkimic.so mic-check libgstiyakkimp3.so libgstiyakkiwebsink.so

.PHONY: install install-camshare install-mic install-mp3 install-websink check clean
