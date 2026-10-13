# IYAKKI web view helper (WPE WebKit). Plain make; Buildroot passes CC / PKG_CONFIG / CFLAGS / LDFLAGS.
#   make            builds ./webview-helper
#   make install DESTDIR=... installs /usr/libexec/iyakki/webview-helper
CC ?= cc
PKG_CONFIG ?= pkg-config
PKGS = wpe-webkit-2.0 wpebackend-fdo-1.0 wayland-server glib-2.0 gio-2.0
CFLAGS ?= -O2
override CFLAGS += -std=gnu11 -Wall -Wextra -Wno-unused-parameter -Wno-array-bounds $(shell $(PKG_CONFIG) --cflags $(PKGS))
LDLIBS += $(shell $(PKG_CONFIG) --libs $(PKGS))

webview-helper: helper.c proto.h
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ helper.c $(LDLIBS)

install: webview-helper
	install -D -m 0755 webview-helper $(DESTDIR)/usr/libexec/iyakki/webview-helper

clean:
	rm -f webview-helper

.PHONY: install clean
