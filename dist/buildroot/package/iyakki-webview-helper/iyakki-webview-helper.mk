################################################################################
#
# iyakki-webview-helper
#
# The offscreen WPE WebKit helper the shell talks to (docs/WEBVIEW-PROTOCOL.md).
# Built from the sources in dist/webview/wpe-helper of this repository.
#
################################################################################

IYAKKI_WEBVIEW_HELPER_VERSION = 1
IYAKKI_WEBVIEW_HELPER_SITE = $(BR2_EXTERNAL_IYAKKI_PATH)/../webview/wpe-helper
IYAKKI_WEBVIEW_HELPER_SITE_METHOD = local
IYAKKI_WEBVIEW_HELPER_LICENSE = Apache-2.0
IYAKKI_WEBVIEW_HELPER_DEPENDENCIES = host-pkgconf wpewebkit wpebackend-fdo libwpe wayland libglib2 mesa3d libdrm gstreamer1 gst1-plugins-base alsa-lib libsoup3

define IYAKKI_WEBVIEW_HELPER_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE1) $(TARGET_CONFIGURE_OPTS) -C $(@D) clean
	$(TARGET_MAKE_ENV) $(MAKE) $(TARGET_CONFIGURE_OPTS) -C $(@D) webview-helper libgstiyakkicamshare.so libgstiyakkimic.so libgstiyakkimp3.so libgstiyakkiwebsink.so
	# The web process extension (custom request headers in the full engine) is not yet proven against the real WPE headers: if it fails to
	# build the image still builds, without it (the helper runs without the extension; the header rules then work in the in-house engine only).
	-$(TARGET_MAKE_ENV) $(MAKE) $(TARGET_CONFIGURE_OPTS) -C $(@D) libiyakkiwebext.so || echo "WARNING: libiyakkiwebext.so did not build; custom headers will not reach the full web engine"
	# The screen portal for getDisplayMedia needs PipeWire at run time and is started only when IYAKKI_WEBVIEW_SCREEN=1: same rule, an image without it still builds
	-$(TARGET_MAKE_ENV) $(MAKE) $(TARGET_CONFIGURE_OPTS) -C $(@D) iyakki-screenportal || echo "WARNING: iyakki-screenportal did not build; screen share stays off"
endef

define IYAKKI_WEBVIEW_HELPER_INSTALL_TARGET_CMDS
ifeq ($(BR2_PACKAGE_IYAKKI_WEBVIEW_HELPER_PACKAGE_ONLY),y)
	# F-BROW-40: the program goes to the package staging folder, not into the image (dist/webview/make-browser-package.sh signs it)
	$(INSTALL) -D -m 0755 $(@D)/webview-helper $(BINARIES_DIR)/browser-package/webview-helper
else
	$(INSTALL) -D -m 0755 $(@D)/webview-helper $(TARGET_DIR)/usr/libexec/iyakki/webview-helper
endif
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkicamshare.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkimic.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkimic.so
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkimp3.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkimp3.so
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkiwebsink.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so
	if [ -f $(@D)/libiyakkiwebext.so ]; then $(INSTALL) -D -m 0755 $(@D)/libiyakkiwebext.so $(TARGET_DIR)/usr/lib/iyakki/webext/libiyakkiwebext.so; fi
	if [ -f $(@D)/iyakki-screenportal ]; then $(INSTALL) -D -m 0755 $(@D)/iyakki-screenportal $(TARGET_DIR)/usr/libexec/iyakki/iyakki-screenportal; fi
endef

$(eval $(generic-package))
