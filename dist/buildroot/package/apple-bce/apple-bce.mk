################################################################################
#
# apple-bce
#
# The t2linux BCE driver (T2 VHCI USB + aaudio sound) for Intel Macs with the Apple T2 chip, built as an out-of-tree module
# against the image's kernel. Only the "mac" variant selects it (configs/iyakki_mac.fragment). See docs/MAC-HARDWARE.md,
# section 6. Our two patches (in this folder, applied in order) make it work on the IYAKKI kernel: one MSI vector without
# interrupt remapping, and ALSA period updates outside the completion handler's spinlock.
#
################################################################################

APPLE_BCE_VERSION = c7448ca0a58299a78836969ddd3b43761ffac6b1
APPLE_BCE_SITE = $(call github,t2linux,apple-bce-drv,$(APPLE_BCE_VERSION))
# MODULE_LICENSE("GPL") in apple_bce.c; the repository has no separate licence file
APPLE_BCE_LICENSE = GPL-2.0

$(eval $(kernel-module))
$(eval $(generic-package))
