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
IYAKKI_WEBVIEW_HELPER_DEPENDENCIES = host-pkgconf wpewebkit wpebackend-fdo libwpe wayland libglib2

define IYAKKI_WEBVIEW_HELPER_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE) $(TARGET_CONFIGURE_OPTS) -C $(@D) clean webview-helper
endef

define IYAKKI_WEBVIEW_HELPER_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/webview-helper $(TARGET_DIR)/usr/libexec/iyakki/webview-helper
endef

$(eval $(generic-package))
