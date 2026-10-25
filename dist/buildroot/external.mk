# Extra Buildroot packages of IYAKKI (only selected by the optional "web" variant; the default image has none).
include $(sort $(wildcard $(BR2_EXTERNAL_IYAKKI_PATH)/package/*/*.mk))

# libgpg-error cross-builds its lock object with gen-lock-obj.sh (objdump + awk). With mawk (the default awk of Debian/Ubuntu
# hosts) that script writes an EMPTY lock object (`_priv[0]`), and every program using libgcrypt/libgpg-error (WebKit's web
# process) dies with "gpgrt fatal: sizeof lock obj". The package ships the right ready-made objects for each target
# (src/syscfg/), the ones Buildroot's BR2_PACKAGE_LIBGPG_ERROR_SYSCFG names: use that one.
define IYAKKI_LIBGPG_ERROR_FIX_LOCK_OBJ
	cp $(@D)/src/syscfg/lock-obj-pub.$(call qstrip,$(BR2_PACKAGE_LIBGPG_ERROR_SYSCFG)).h $(@D)/src/lock-obj-pub.native.h
	rm -f $(@D)/src/gpg-error.h
endef
LIBGPG_ERROR_POST_CONFIGURE_HOOKS += IYAKKI_LIBGPG_ERROR_FIX_LOCK_OBJ

# host-mesa3d (built for iris/crocus: it makes mesa_clc and the other build tools) links the HOST LLVM, which Buildroot builds without
# RTTI, and Mesa's meson.build refuses to build when its own RTTI setting differs from LLVM's ("LLVM was built without RTTI, so Mesa must
# also disable RTTI"). Buildroot's host-mesa3d does not pass the switch (the target package does, from BR2_PACKAGE_LLVM_RTTI).
HOST_MESA3D_CONF_OPTS += -Dcpp_rtti=false

# The target Mesa asks `llvm-config --has-rtti`; the llvm-config in the staging tree is the HOST llvm's (built before RTTI was switched on for
# iris, and Buildroot does not rebuild host-llvm for that), so it answers NO although the target LLVM libraries are built with RTTI (the
# staging LLVMConfig.cmake says ON) and Mesa is built with -Dcpp_rtti=true: Mesa's meson.build then stops on the mismatch. Put a thin wrapper
# in front of llvm-config that answers YES for --has-rtti and forwards everything else.
define IYAKKI_MESA3D_LLVM_CONFIG_RTTI
	if [ -x $(STAGING_DIR)/usr/bin/llvm-config ] && [ ! -e $(STAGING_DIR)/usr/bin/llvm-config.real ] && grep -q 'LLVM_ENABLE_RTTI ON' $(STAGING_DIR)/usr/lib/cmake/llvm/LLVMConfig.cmake 2>/dev/null; then \
		mv $(STAGING_DIR)/usr/bin/llvm-config $(STAGING_DIR)/usr/bin/llvm-config.real; \
		printf '#!/bin/sh\ncase " $$* " in *" --has-rtti "*) echo YES; exit 0;; esac\nexec "$$0.real" "$$@"\n' > $(STAGING_DIR)/usr/bin/llvm-config; \
		chmod 755 $(STAGING_DIR)/usr/bin/llvm-config; \
	fi
endef
MESA3D_PRE_CONFIGURE_HOOKS += IYAKKI_MESA3D_LLVM_CONFIG_RTTI
