# Security policy

## Supported versions

The latest released version gets security fixes.

## Reporting a vulnerability

Use the private **Report a vulnerability** feature on the GitHub Security tab.
Do not open a public issue for a possible security defect.

Include:

- Include the affected version and operating system.
- Include steps that use placeholder credentials.
- Include the expected and observed behavior.
- Include a proposed fix when one is available.

Never include real Claude credentials, access tokens, refresh tokens, API keys,
or the contents of `.credentials.json`.

## Security model

Claude Code owns OAuth login, credential storage, token refresh, and logout.
The manager does not parse, copy, or store OAuth credential values.
It reads only `oauthAccount.emailAddress` from the local Claude account file.

The default profile keeps the standard Claude Code config and Keychain locations.
Each isolated profile uses one private config location.
The manager supports only audited Claude Code 2.1.226.

The manager checks the subscription identity before each normal launch.
It applies authentication-related launch settings to the same check.
It removes inherited authentication and provider variables from managed processes.

Profile and reservation directories use mode `0700`.
State, lock, and reservation files use mode `0600`.
Managed directory paths cannot contain symlink components.

Version 1 isolated OAuth profiles can migrate when their account email is present.
The manager rejects imported API profiles without reading their API keys.

The program uses the permissions of the current user.
Use a private local directory for `CLAUDE_ACCOUNT_HOME`.
Filesystem ACLs and network shares can grant access despite POSIX mode bits.

See [SECURITY-REVIEW.md](SECURITY-REVIEW.md) for the reviewed controls and tests.
