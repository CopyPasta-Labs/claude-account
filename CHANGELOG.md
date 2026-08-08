# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

This fork will use version 0.3.0.

### Added

- Add a default profile location that keeps the standard Claude Code Keychain entry.
- Add `adopt-default` and `reauth` account commands.
- Store and verify the expected subscription email for each profile.
- Set `ANTHROPIC_CONFIG_DIR` for default and isolated profiles.

### Changed

- Require the audited Claude Code 2.1.226 release on all supported systems.
- Verify the authentication method, provider, email, subscription type, and local account email.
- Remove inherited authentication, provider, endpoint, gateway, and host-auth variables.
- Remove inherited session credentials and account identity selectors.
- Apply authentication-related launch settings to the identity preflight.
- Reject `--bare` because it disables subscription OAuth.
- Migrate version 1 isolated profiles to state version 2 when an account email exists.
- Reject `remove --purge` until profile deletion is transactional.

### Security

- Reject symlink components in managed directory paths.
- Open state and lock files with no-follow protection.
- Validate all loaded profile paths and metadata.
- Serialize installation and reject recursive real-Claude paths.
- Pin one canonical real-Claude path for each operation.
- Limit the Claude version probe on Linux and macOS.
- Serialize each launch with login and logout changes for that profile.
- Sync parent directories after atomic state writes.
- Reset existing state and lock file permissions.
- Pin GitHub Actions to full commit identifiers.
- Remove the unsigned binary release workflow.

## [0.2.0] - 2026-08-04

### Added

- macOS account isolation with a Claude Code 2.1.144 minimum-version guard for
  profile-scoped Keychain credentials.
- Native CI and release archives for Apple Silicon and Intel macOS.
- An explicit `resolve-case-collision` recovery command for legacy state that
  contains names differing only by ASCII letter case.
- Native macOS storage under `~/Library/Application Support/claude-account`,
  compatible with OAuth profiles created by `Kerber0ss/claude-account-macos`.
- Automatic reuse of an existing XDG-style macOS installation when it is the
  only existing account registry.

### Changed

- Reject macOS profile names that differ only by ASCII letter case to prevent
  collisions on case-insensitive filesystems.
- Force managed Claude processes to use the selected profile's configuration
  and secure-storage directories, and ignore inherited authentication tokens
  unless `CLAUDE_ACCOUNT_PRESERVE_AUTH_ENV=1` is set.
- Print shell startup guidance for both zsh and Bash.

### Fixed

- Serialize `account use` with profile removal so concurrent commands cannot
  leave the active profile pointing to a removed account.
- Fail closed when both native and XDG-style macOS account registries exist,
  rather than selecting one silently.
- Detect API profiles created by `claude-account-macos` and reject them without
  reading or migrating their API keys; OAuth profiles remain compatible.

### Contributors

- macOS behavior and native layout were validated by
  [@Kerber0ss](https://github.com/Kerber0ss).
- Cross-platform isolation and test hardening were contributed by
  [@Yiminnn](https://github.com/Yiminnn).

## [0.1.1] - 2026-07-30

### Fixed

- Complete Claude Code onboarding after a verified `account add` login so the
  first normal `claude` launch does not ask the user to authenticate again.
- Require `auth status --json` to explicitly report `loggedIn: true` before a
  profile is registered.

## [0.1.0] - 2026-07-30

### Added

- Linux-only Claude Code profile isolation through `CLAUDE_CONFIG_DIR`.
- `add`, `use`, `list`, `current`, and `remove` account commands.
- Transparent forwarding of normal Claude Code commands and arguments.
- Official Claude Code login, status verification, and logout integration.
- Atomic state writes, process locking, strict filesystem permissions, and
  profile-name validation.
- Safe profile removal with separate unregister and permanent purge modes.
- Non-invasive shim installation that preserves the official Claude launcher.
- Unit and end-to-end lifecycle tests.

[Unreleased]: https://github.com/CopyPasta-Labs/claude-account/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/CopyPasta-Labs/claude-account/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/CopyPasta-Labs/claude-account/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/CopyPasta-Labs/claude-account/releases/tag/v0.1.0
