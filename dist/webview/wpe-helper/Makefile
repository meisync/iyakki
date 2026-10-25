# IYAKKI web view helper (WPE WebKit). Plain make; Buildroot passes CC / PKG_CONFIG / CFLAGS / LDFLAGS.
#   make            builds ./webview-helper
#   make install DESTDIR=... installs /usr/libexec/iyakki/webview-helper
CC ?= cc
PKG_CONFIG ?= pkg-config
PKGS = wpe-webkit-2.0 wpebackend-fdo-1.0 wayland-server egl glesv2 gbm glib-2.0 gio-2.0
CFLAGS ?= -O2
override CFLAGS += -std=gnu11 -Wall -Wextra -Wno-unused-parameter -Wno-array-bounds $(shell $(PKG_CONFIG) --cflags $(PKGS))
LDLIBS += $(shell $(PKG_CONFIG) --libs $(PKGS))

# media-probe.js becomes a C string (media-probe.h) so the helper needs no data file at run time
media-probe.h: media-probe.js
	sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/^/"/' -e 's/$$/\\n"/' media-probe.js > $@

webview-helper: helper.c proto.h media-probe.h
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ helper.c $(LDLIBS)

install: webview-helper
	install -D -m 0755 webview-helper $(DESTDIR)/usr/libexec/iyakki/webview-helper

clean:
	rm -f webview-helper media-probe.h

.PHONY: install clean
