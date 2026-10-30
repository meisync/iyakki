################################################################################
#
# iyakki-web-media (no package: one setting for wpewebkit)
#
# Camera, microphone, screen share and WebRTC for pages (WEB_MEDIA=1, configs/iyakki_web_media.fragment; docs/development/BROWSER-MEDIA-DEVICES.md).
# Buildroot's wpewebkit package passes -DENABLE_WEB_RTC=OFF and has no option for it; a later -D wins in CMake. WebKit then uses GStreamer's
# webrtcbin (USE_GSTREAMER_WEBRTC is on in the WPE port; it needs OpenSSL 3 and gstreamer-webrtc, both in the media build).
# Off unless build.sh exported IYAKKI_WEB_MEDIA=1. Kept in its own file so that external.mk (whose change makes changed.sh ask for a full build) stays as it was.
#
################################################################################

ifeq ($(IYAKKI_WEB_MEDIA),1)
WPEWEBKIT_CONF_OPTS += -DENABLE_WEB_RTC=ON
# MediaRecorder: WebKit's recorder pipeline is `uritranscodebin` (gst-plugins-bad's `transcode` plugin, which Buildroot builds with
# -Dtranscode=disabled; a later -D wins in meson) over `encodebin` (gst-plugins-base `encoding`, configs/iyakki_web_media.fragment).
# Without them every recorder page fails with NotSupportedError ("The MediaRecorder is unsupported on this platform").
GST1_PLUGINS_BAD_CONF_OPTS += -Dtranscode=enabled
endif
