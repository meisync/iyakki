################################################################################
#
# nvidia-open
#
# NVIDIA's open GPU kernel modules (github.com/NVIDIA/open-gpu-kernel-modules, dual MIT / GPL-2.0) built out of tree against the
# image's kernel: nvidia.ko, nvidia-uvm.ko, nvidia-modeset.ko, nvidia-drm.ko. They load from /lib/modules/<kernel>/updates/ like
# apple-bce, and the post-build step signs them with the others when the image is Secure-Boot-signed (docs/SECURE-BOOT.md).
#
# NEVER selected by default: configs/iyakki_nvidia_open.fragment turns it on (PC and Mac images). Only the kernel modules are here.
# The vendor user-space libraries and the GSP firmware that must match this exact version are downloaded for the person by the
# installer (crates/iyakki-agent/src/gpu_nvidia.rs, NVIDIA_VERSION); the test `nvidia_version_pins_agree` keeps the two in step.
# This version is also the release tag of NVIDIA's .run file, so the module and the libraries cannot drift apart.
#
# The kernel-module infrastructure is not used for the build: NVIDIA's top Makefile builds the closed-form objects nv-kernel.o and
# nv-modeset-kernel.o with the compiler first, then runs kbuild itself with two stages (the *-linux.o link needs the modpost
# output). Building the modules with a plain `M=` call would skip that link. So: the top Makefile builds, and the modules are
# installed with the kernel's own modules_install (into updates/), exactly like the infrastructure would.
#
################################################################################

NVIDIA_OPEN_VERSION = 595.99.02
NVIDIA_OPEN_SITE = $(call github,NVIDIA,open-gpu-kernel-modules,$(NVIDIA_OPEN_VERSION))
NVIDIA_OPEN_LICENSE = MIT or GPL-2.0
NVIDIA_OPEN_LICENSE_FILES = COPYING
NVIDIA_OPEN_DEPENDENCIES = linux $(BR2_MAKE_HOST_DEPENDENCY)

ifeq ($(BR2_PACKAGE_NVIDIA_OPEN),y)
LINUX_NEEDS_MODULES = y
endif

# ARCH=x86_64 (not the kernel's "x86"): NVIDIA's Makefile wants the machine name. SYSSRC/SYSOUT: the kernel build tree. HOST_CC builds
# the small generators that run on the build machine.
NVIDIA_OPEN_MAKE_OPTS = \
	ARCH=x86_64 \
	CC="$(TARGET_CC)" \
	LD="$(TARGET_LD)" \
	OBJDUMP="$(TARGET_OBJDUMP)" \
	HOST_CC="$(HOSTCC)" \
	SYSSRC=$(LINUX_DIR) \
	SYSOUT=$(LINUX_DIR)

define NVIDIA_OPEN_BUILD_CMDS
	$(LINUX_MAKE_ENV) $(BR2_MAKE) -C $(@D) $(NVIDIA_OPEN_MAKE_OPTS) modules
endef

# Only the four modules (no nvidia-peermem, no vGPU): the same list the vendor installer gives a desktop card.
define NVIDIA_OPEN_INSTALL_TARGET_CMDS
	$(LINUX_MAKE_ENV) $(BR2_MAKE) -C $(LINUX_DIR) $(LINUX_MAKE_FLAGS) \
		M=$(@D)/kernel-open PWD=$(@D)/kernel-open \
		modules_install
endef

$(eval $(generic-package))
