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
IYAKKI_WEBVIEW_HELPER_DEPENDENCIES = host-pkgconf wpewebkit wpebackend-fdo libwpe wayland libglib2 mesa3d libdrm gstreamer1 gst1-plugins-base alsa-lib

define IYAKKI_WEBVIEW_HELPER_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE1) $(TARGET_CONFIGURE_OPTS) -C $(@D) clean
	$(TARGET_MAKE_ENV) $(MAKE) $(TARGET_CONFIGURE_OPTS) -C $(@D) webview-helper libgstiyakkicamshare.so libgstiyakkimic.so libgstiyakkimp3.so libgstiyakkiwebsink.so
endef

define IYAKKI_WEBVIEW_HELPER_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/webview-helper $(TARGET_DIR)/usr/libexec/iyakki/webview-helper
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkicamshare.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkicamshare.so
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkimic.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkimic.so
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkimp3.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkimp3.so
	$(INSTALL) -D -m 0755 $(@D)/libgstiyakkiwebsink.so $(TARGET_DIR)/usr/lib/gstreamer-1.0/libgstiyakkiwebsink.so
endef

$(eval $(generic-package))
