#!/bin/sh
# Create the test users (test-only passwords; see tests/support/mod.rs).
set -eu
KEYS=/tmp/keys

add_user() { # name shell password
    useradd -m -s "$2" "$1"
    echo "$1:$3" | chpasswd
    install -d -m 700 -o "$1" -g "$1" "/home/$1/.ssh"
}

authorize() { # user key...
    user=$1; shift
    : > "/home/$user/.ssh/authorized_keys"
    for k in "$@"; do cat "$KEYS/$k.pub" >> "/home/$user/.ssh/authorized_keys"; done
    chown "$user:$user" "/home/$user/.ssh/authorized_keys"
    chmod 600 "/home/$user/.ssh/authorized_keys"
}

add_user tester /bin/bash tester-password
authorize tester id_rsa_openssh id_rsa_pem id_ed25519_enc
# Secondary group of tester (SFTP save keeps a file's non-primary group, §6.4).
groupadd shellfin-shared
usermod -aG shellfin-shared tester

add_user tester2fa /bin/bash tester2fa-password
authorize tester2fa id_ed25519
# TOTP secret JBSWY3DPEHPK3PXP; no RATE_LIMIT / DISALLOW_REUSE (tests may log in repeatedly).
printf 'JBSWY3DPEHPK3PXP\n" TOTP_AUTH\n" WINDOW_SIZE 3\n' > /home/tester2fa/.google_authenticator
chown tester2fa:tester2fa /home/tester2fa/.google_authenticator
chmod 400 /home/tester2fa/.google_authenticator

add_user tester-fish /usr/bin/fish tester-fish-password
authorize tester-fish id_rsa_openssh id_ed25519_enc

add_user tester-tcsh /usr/bin/tcsh tester-tcsh-password
authorize tester-tcsh id_rsa_openssh id_ed25519_enc

add_user tester3 /bin/bash tester3-password
authorize tester3 id_rsa_openssh id_rsa_pem id_ed25519_enc

# SFTP-only account: ForceCommand internal-sftp makes the identity probe's `id -u; id -G` exec run
# the SFTP server, which waits on stdin.
add_user tester-sftponly /bin/bash tester-sftponly-password
authorize tester-sftponly id_rsa_openssh

rm -rf "$KEYS"
