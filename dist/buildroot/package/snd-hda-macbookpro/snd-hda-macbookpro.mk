################################################################################
#
# snd-hda-macbookpro
#
# davidjo's snd_hda_macbookpro: sound output for Macs with the Cirrus Logic CS8409 HD-audio bridge and Apple's amplifiers
# (MacBookPro13,x/14,x without the T2 chip, iMacs 2017-2019), built out of tree against the image's kernel as
# snd-hda-codec-cs8409.ko. Only the "mac" variant selects it (configs/iyakki_mac.fragment). See docs/MAC-HARDWARE.md.
#
# How it is made: the project is not a standalone driver but three diffs against the kernel's sound/pci/hda/patch_cs8409.{c,h}
# plus its own include files (patch_cirrus/*.h, the Apple codec setup). Its install script downloads a kernel tarball; here the
# kernel tree of the image's own build is the base, so the module matches the kernel it runs in. The post-patch hook copies the
# three kernel files, applies the project's diffs (they apply to 6.12 with offsets only), and puts our Kbuild file in place of the
# project's Makefile (the project builds with CONFIG_SND_HDA_CODEC_CS8409, which this kernel does not set).
# The module carries the alias hdaudio:v10138409r*a01*: the HD-audio bus asks modprobe for it when the codec appears, before the
# generic driver is tried, so no explicit load is needed when modules.dep is current (iyakki-drivers' cs8409 checks it anyway).
# On a CS8409 whose subsystem id is not one of Apple's the module returns -ENODEV ("UNKNOWN subsystem id") and the generic driver binds.
# iyakki-apple-ssid-first.diff (ours, applied last, not named .patch so Buildroot does not apply it before the hook above) makes the
# module test the Apple subsystem ids FIRST (0x106b3300/3600/3900/0e00/0f00/1000) and call the Apple code directly, and logs with
# pr_info which path ran, what the generic parser found (DACs, ADCs), the boot setup result and the PCMs built (dmesg | grep cs8409:).
#
################################################################################

SND_HDA_MACBOOKPRO_VERSION = 89b22ff90b86468b186706861dd18663562defa7
SND_HDA_MACBOOKPRO_SITE = $(call github,davidjo,snd_hda_macbookpro,$(SND_HDA_MACBOOKPRO_VERSION))
# LICENSE is the GPL-2.0 text; the files derived from the kernel (patch_cs8409.*) are GPL-2.0 too (MODULE_LICENSE("GPL"))
SND_HDA_MACBOOKPRO_LICENSE = GPL-2.0
SND_HDA_MACBOOKPRO_LICENSE_FILES = LICENSE

define SND_HDA_MACBOOKPRO_DERIVE_FROM_KERNEL
	for f in patch_cs8409.c patch_cs8409.h patch_cs8409-tables.c; do \
		cp $(LINUX_DIR)/sound/pci/hda/$$f $(@D)/$$f || exit 1; \
	done
	cp $(@D)/patch_cirrus/patch_cirrus_*.h $(@D)/
	cd $(@D) && for d in patch_patch_cs8409.c.diff patch_patch_cs8409.h.diff patch_patch_cirrus_apple.h.diff; do \
		patch -p2 -b < $$d || exit 1; \
	done
	cd $(@D) && patch -p1 -b < $(SND_HDA_MACBOOKPRO_PKGDIR)/iyakki-apple-ssid-first.diff
	cp $(SND_HDA_MACBOOKPRO_PKGDIR)/Kbuild $(@D)/Kbuild
endef
SND_HDA_MACBOOKPRO_POST_PATCH_HOOKS += SND_HDA_MACBOOKPRO_DERIVE_FROM_KERNEL

$(eval $(kernel-module))
$(eval $(generic-package))
