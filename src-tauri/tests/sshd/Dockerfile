# Shellfin integration-test sshd (§10.2). Build context: src-tauri/tests (see compose.yml).
FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      openssh-server libpam-google-authenticator fish tcsh bash procps \
 && rm -rf /var/lib/apt/lists/* \
 && rm -f /etc/ssh/ssh_host_*

COPY sshd/sshd_config /etc/ssh/sshd_config
COPY sshd/pam-sshd /etc/pam.d/sshd
COPY sshd/banner.txt /etc/ssh/banner.txt
COPY fixtures/keys/*.pub /tmp/keys/
COPY sshd/setup-users.sh /usr/local/sbin/setup-users.sh
COPY sshd/entrypoint.sh /usr/local/sbin/entrypoint.sh
RUN sed -i 's/\r$//' /usr/local/sbin/setup-users.sh /usr/local/sbin/entrypoint.sh /etc/ssh/sshd_config /etc/pam.d/sshd \
 && chmod 755 /usr/local/sbin/setup-users.sh /usr/local/sbin/entrypoint.sh \
 && /usr/local/sbin/setup-users.sh

EXPOSE 22
ENTRYPOINT ["/usr/local/sbin/entrypoint.sh"]
