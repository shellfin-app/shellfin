// Hints shown to the user for copy & paste (docs/ARCHITECTURE.md §5.3). The app never runs them. Commands that
// contain local paths or lookup names are built by the backend; only the fixed server-side
// fingerprint command is derived here.

/** `ssh-keygen -lf /etc/ssh/ssh_host_<type>_key.pub` hint for the server side of the fingerprint comparison. */
export function serverFingerprintCommand(keyType: string): string {
  const family = /^ssh-(ed25519|rsa|dss)/.exec(keyType)?.[1] ?? (keyType.startsWith("ecdsa") ? "ecdsa" : "ed25519");
  return `ssh-keygen -lf /etc/ssh/ssh_host_${family}_key.pub`;
}
