# Contributing

Thank you for helping improve claude-account.

## Before opening a change

- Search existing issues and pull requests.
- Use a GitHub issue for behavior changes that would alter the command-line
  interface or stored state format.
- Never post Claude credentials, access tokens, refresh tokens, API keys, or
  the contents of `.credentials.json`.

## Development setup

Requirements:

- Linux or macOS
- Rust 1.85.1
- Xcode Command Line Tools when building on macOS
- Claude Code 2.1.226 for real authentication tests

Run the local checks:

```bash
cargo fmt --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

The automated tests use temporary directories and a fake Claude executable.
They must not read or modify the real `~/.claude` directory of the developer.

Test authentication, secure storage, profile paths, and migration with two real accounts before release.
Never put real credentials in tests, logs, issues, or pull requests.

## Pull requests

- Keep changes focused.
- Add or update tests for behavioral changes.
- Update README.md and CHANGELOG.md when the user-facing behavior changes.
- Preserve backward compatibility for `state.json`, or include an explicit
  migration.
- Explain destructive filesystem behavior clearly and test its path guards.

The MIT License applies to each contribution.
