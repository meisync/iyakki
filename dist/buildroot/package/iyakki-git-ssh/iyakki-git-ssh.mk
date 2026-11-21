################################################################################
#
# iyakki-git-ssh
#
# OpenSSH's `ssh` client alone (no sshd, no scp, no ssh-agent, no keygen): git reaches the in-memory ssh-agent that IYAKKI serves
# through it. Installed as /usr/libexec/iyakki/git-ssh so that Dropbear stays the `ssh` of the PATH. docs/GIT-AND-SSH.md
#
################################################################################

IYAKKI_GIT_SSH_VERSION = 10.2p1
IYAKKI_GIT_SSH_SOURCE = openssh-$(IYAKKI_GIT_SSH_VERSION).tar.gz
IYAKKI_GIT_SSH_SITE = http://ftp.openbsd.org/pub/OpenBSD/OpenSSH/portable
IYAKKI_GIT_SSH_LICENSE = BSD-3-Clause, BSD-2-Clause, ISC, Public Domain
IYAKKI_GIT_SSH_LICENSE_FILES = LICENCE
IYAKKI_GIT_SSH_DEPENDENCIES = host-pkgconf zlib openssl
IYAKKI_GIT_SSH_CONF_ENV = \
	LD="$(TARGET_CC)" \
	LDFLAGS="$(TARGET_CFLAGS)" \
	LIBS=`$(PKG_CONFIG_HOST_BINARY) --libs openssl`
IYAKKI_GIT_SSH_CONF_OPTS = \
	--sysconfdir=/etc/ssh \
	--without-sandbox \
	--without-pam \
	--without-audit \
	--without-ssl-engine \
	--disable-lastlog --disable-utmp --disable-utmpx --disable-wtmp --disable-wtmpx \
	--disable-strip

# `ssh -J` starts the jump hop with the program at its compiled-in path (`_PATH_SSH_PROGRAM`, /usr/bin/ssh), which on an image is Dropbear's
# client. The path is made this very program, and the build stops when the source no longer has the line that is changed (KI-112).
define IYAKKI_GIT_SSH_JUMP_PROGRAM
	sed -i 's|-D_PATH_SSH_PROGRAM=[^ ]*||' $(@D)/Makefile.in
	sed -i 's|"/usr/bin/ssh"|"/usr/libexec/iyakki/git-ssh"|' $(@D)/pathnames.h
	grep -q '"/usr/libexec/iyakki/git-ssh"' $(@D)/pathnames.h
endef
IYAKKI_GIT_SSH_POST_PATCH_HOOKS += IYAKKI_GIT_SSH_JUMP_PROGRAM

define IYAKKI_GIT_SSH_BUILD_CMDS
	$(TARGET_MAKE_ENV) $(MAKE) -C $(@D) ssh
endef

define IYAKKI_GIT_SSH_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/ssh $(TARGET_DIR)/usr/libexec/iyakki/git-ssh
endef

$(eval $(autotools-package))
