#!/bin/sh
# Generate host keys on first start (tests may regenerate them later), then run sshd in the
# foreground, logging to stderr (docker logs).
set -e
mkdir -p /run/sshd
ssh-keygen -A
exec /usr/sbin/sshd -D -e
