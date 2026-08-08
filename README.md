# claude-account

[![CI](https://github.com/CopyPasta-Labs/claude-account/actions/workflows/ci.yml/badge.svg)](https://github.com/CopyPasta-Labs/claude-account/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

`claude-account` switches the subscription account that new Claude Code processes use.
The switch does not copy, read, refresh, or store OAuth credentials.
The official Claude Code executable owns authentication and token refresh.

This fork supports one default account and multiple isolated accounts.
The default account keeps Claude Code's standard config files and Keychain entry.
Each isolated account uses a fixed private config directory.

```bash
claude account adopt-default main --email main@example.com
claude account add second --email second@example.com

claude account use main
claude account use second
claude account current
claude account list
```

The first login for an isolated account can require browser approval.
Later switches do not require interaction while both OAuth sessions remain valid.

## Documentation

- [What this fork provides and how to use it](docs/USAGE.md)
- [Complete fork changes and reasons](docs/FORK-CHANGES.md)

> [!IMPORTANT]
> This community project is not made, endorsed, or supported by Anthropic.
> Claude and Claude Code are Anthropic products.

## Requirements

- Linux or macOS
- Claude Code 2.1.226 exactly
- A valid Claude Pro, Max, Team, or Enterprise subscription for each profile
- Rust 1.85.1 to build this source

This program supports the terminal CLI.
It does not switch the graphical Claude Code extension for VS Code.

## Build and install

This fork does not publish binary releases. Build the reviewed source locally.

```bash
git clone https://github.com/CopyPasta-Labs/claude-account.git
cd claude-account
cargo build --locked --release
./target/release/claude-account install
```

The installer prints the shim directory.
Put that directory before the official Claude Code directory in `PATH`.

```bash
type -a claude
claude account list
```

The `claude-account` shim must appear before the official `claude` executable.
The installer rejects a real-Claude path that resolves to the manager or its shim.

## Set up two subscriptions

Use this procedure when the current default Claude Code login is the first account.

1. Register the default account.

   ```bash
   claude account adopt-default main --email main@example.com
   ```

2. Add the second account.

   ```bash
   claude account add second --email second@example.com
   ```

3. Complete the official OAuth flow when Claude Code opens it.

4. Select an account before you start a new Claude process.

   ```bash
   claude account use main
   claude account use second
   ```

Existing Claude processes keep their original accounts.
Only new processes use the newly selected profile.

## Commands

### Register the default account

```bash
claude account adopt-default NAME --email EMAIL
```

This command does not start a login.
It verifies the current default Claude Code subscription and registers it.
Only one profile can use the default location.

### Add an isolated account

```bash
claude account add NAME --email EMAIL
claude account add COMPANY --email EMAIL --sso
```

The email is required.
Claude Code runs its official subscription login flow.
The program registers the profile only after all identity checks pass.

The command preserves the profile directory after a failed login.
Use the preserved directory to diagnose the failure or retry.

### Select and inspect profiles

```bash
claude account use NAME
claude account list
claude account current
```

`current` prints only the active profile name.

### Repair a login

```bash
claude account reauth NAME
claude account reauth NAME --sso
```

This command runs the official login flow for the stored email.
It bypasses the normal preflight check, then verifies the new login.

These direct recovery commands also bypass the normal preflight check:

```bash
claude auth login
claude auth logout
claude auth status --json
```

### Remove a profile

```bash
claude account remove NAME
```

This command runs Claude Code's official logout and unregisters the profile.
It preserves settings, sessions, plugins, and history.

The program currently rejects `remove --purge`.
Profile deletion will remain disabled until the delete operation is transactional.

## Identity checks

The program runs `claude auth status --json` before each normal Claude launch.
It requires all these values:

```text
loggedIn = true
authMethod = claude.ai
apiProvider = firstParty
email = the profile email
subscriptionType = pro, max, team, or enterprise
```

The program also checks `oauthAccount.emailAddress` in Claude's local account file.
That email must match the status output and the registered profile.

These checks reject provider credentials, API-key helpers, wrong accounts, and logged-out sessions.
The preflight uses the launch values for `--settings`, `--setting-sources`, and `--safe-mode`.
The manager rejects `--bare` because that option disables subscription OAuth.
The program does not delete or edit a profile's Claude settings.

Claude can reload settings and managed policy during a running session.
The manager verifies the identity at process start and does not monitor later changes.

## Profile locations

The default profile uses these paths:

```text
~/.claude/
~/.claude.json
~/.claude/.anthropic/
```

The manager unsets `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR` for this profile.
Claude Code continues to use its default Keychain entry.

An isolated profile uses these variables:

```text
CLAUDE_CONFIG_DIR=<profile>
CLAUDE_SECURESTORAGE_CONFIG_DIR=<profile>
ANTHROPIC_CONFIG_DIR=<profile>/.anthropic
```

Linux stores manager data in these default locations:

```text
~/.config/claude-account/state.json
~/.local/share/claude-account/profiles/<name>/
~/.local/share/claude-account/bin/claude
~/.local/share/claude-account/libexec/claude-account
```

macOS stores manager data in this default location:

```text
~/Library/Application Support/claude-account/
```

`XDG_CONFIG_HOME` and `XDG_DATA_HOME` can change the manager locations.
`CLAUDE_ACCOUNT_HOME` can set one absolute root for tests or custom installations.

The state file contains profile names, expected emails, profile locations, and the real executable path.
It does not contain OAuth tokens, API keys, or Keychain data.

## Authentication environment

The manager removes inherited authentication, provider, endpoint, gateway, and host-auth variables.
It preserves unrelated shell, proxy, certificate, cloud-tool, and session variables.

There is no environment override that disables this filtering.
See [the security review](SECURITY-REVIEW.md) for the reviewed variable classes.

## State migration

State version 2 stores the expected email and the profile location.
Version 1 isolated profiles migrate when their `.claude.json` file contains `oauthAccount.emailAddress`.
The program rejects a migration when it cannot identify the stored account.

The program rejects imported API profiles without reading or migrating their API keys.

## Development

```bash
cargo fmt --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

The manager rejects unaudited Claude Code versions.
GitHub Actions use commit-pinned actions.
See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).

## License

This project uses the [MIT License](LICENSE).
