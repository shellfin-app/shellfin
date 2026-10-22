# Security Policy

## Supported versions

Only the latest stable release (`vX.Y.Z`) is supported; security fixes ship as a new stable release. Experimental
pre-releases (`vX.Y.Z-experimental.N`) are not patched themselves: fixes reach them through a newer pre-release or a
newer stable release.

## Reporting a vulnerability

Please **do not open a public issue** for security problems.

Preferred: use GitHub private vulnerability reporting. Open the repository's **Security** tab, choose
**Report a vulnerability**, and fill in the form
(<https://github.com/shellfin-app/shellfin/security/advisories/new>).

Alternative: email **security@shellfin.app**.

Please include, as far as you can:

- the affected version or commit, operating system and how you installed or built the app;
- a description of the issue and its impact (what can an attacker do, and what do they need to be able to do first);
- step-by-step reproduction, a proof of concept, or a failing test;
- relevant logs or screenshots, with hostnames, usernames, IP addresses and credentials **redacted**. Never send
  private keys or passwords;
- whether the issue is already public or you plan to disclose it, and any deadline you have.

## What to expect

- We acknowledge your report within **7 days**.
- We give you a status update (triage result, severity, planned fix) within **14 days**.
- We aim to follow **coordinated disclosure with a 90-day window**: we will work with you on a fix and publish an
  advisory, crediting you if you wish, no later than 90 days after the report. If a fix needs longer, we will agree
  an extension with you; if the issue is being actively exploited we may disclose sooner.

These are targets set by volunteers, not guarantees.

## Scope

In scope:

- the Shellfin application and its build and release configuration;
- the IPC layer and the Tauri ACL/capabilities, CSP and navigation restrictions (for example, anything that lets web
  content reach a command or permission it should not);
- SSH handling: host-key verification and `known_hosts` handling, authentication, `ssh_config` parsing, ssh-agent use;
- SFTP and remote/local path handling, including file transfer and the editor's save logic;
- storage and handling of secrets (keychain use, memory handling, logging);
- the updater: anything that makes Shellfin install an update not signed with the release key, an older version
  than announced, or that reaches the update commands from web content.

Out of scope:

- vulnerabilities in upstream dependencies (russh, Tauri, the OS WebView and so on). Please report them to the
  upstream project as well; tell us if they affect Shellfin in a way we can mitigate;
- social engineering of maintainers or users, and phishing;
- denial of service that requires local administrator/root access, or physical access to an unlocked machine;
- issues that require an already-compromised user account or a malicious local administrator;
- findings from automated scanners without a demonstrated impact on Shellfin.

## Safe harbour

We consider good-faith security research conducted under this policy to be authorized. If you make a good-faith
effort to avoid privacy violations, data destruction and service disruption, only test against systems and accounts
you own or have permission to use, and report to us privately and promptly, we will not pursue or support legal action
against you for that research, and we will work with you to understand and resolve the issue. This policy cannot
authorize testing against systems that belong to third parties.
