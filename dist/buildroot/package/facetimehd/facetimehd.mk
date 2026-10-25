################################################################################
#
# facetimehd
#
# patjak's facetimehd: driver of the Broadcom 720p FaceTime HD camera (PCI 14e4:1570, "multimedia controller") of the MacBookPro13,x/14,x
# and other non-T2 MacBooks. Built out of tree against the image's kernel (the Mac variant only: configs/iyakki_mac.fragment).
# Pinned to master of 2026-10-14 (PR #355), which builds clean against Linux 6.12.112 with no patch. Its Makefile builds against
# /lib/modules/$(uname -r); our Kbuild file replaces it. The driver needs videobuf2-dma-sg, which this kernel does not build, so the
# kernel's own videobuf2-dma-sg.c is copied next to it and built as a second module (no kernel rebuild; see Kbuild).
# The firmware (facetimehd/firmware.bin, sensor files *_01XX.dat) is Apple's and is NOT part of this package; IYAKKI's "Camera firmware"
# job makes it on the person's machine (docs/MAC-CAMERA.md). Without it the module fails to load the firmware and no /dev/video0 appears.
# Alias: pci:v000014E4d00001570*, so the PCI core asks modprobe for it only on that camera.
#
################################################################################

FACETIMEHD_VERSION = 6dc8cd85e87993ff21cb01eb333ff42afb058551
FACETIMEHD_SITE = $(call github,patjak,facetimehd,$(FACETIMEHD_VERSION))
FACETIMEHD_LICENSE = GPL-2.0
FACETIMEHD_LICENSE_FILES = LICENSE

define FACETIMEHD_DERIVE_FROM_KERNEL
	cp $(LINUX_DIR)/drivers/media/common/videobuf2/videobuf2-dma-sg.c $(@D)/videobuf2-dma-sg.c
	cp $(FACETIMEHD_PKGDIR)/Kbuild $(@D)/Kbuild
	rm -f $(@D)/Makefile
endef
FACETIMEHD_POST_PATCH_HOOKS += FACETIMEHD_DERIVE_FROM_KERNEL

$(eval $(kernel-module))
$(eval $(generic-package))
