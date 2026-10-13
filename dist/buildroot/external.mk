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
