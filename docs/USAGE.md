# What this fork provides and how to use it

## What you have

This fork provides a terminal account manager for Claude Code subscriptions.
It keeps one selected profile for all new Claude Code processes.

The manager supports two profile locations:

- A default profile uses the standard Claude Code files and Keychain entry.
- An isolated profile uses a private config directory and a separate Keychain entry.

Official Claude Code performs login, logout, credential storage, and token refresh.
The manager does not read, parse, copy, refresh, or store OAuth tokens.

The manager stores these profile facts:

- The profile name
- The expected account email
- The profile location
- The real Claude Code executable path
- The active profile name

The manager checks the selected identity before each normal launch.
It stops the launch when Claude reports the wrong account or authentication method.

## Supported use

The fork supports:

- The Claude Code terminal CLI
- Claude Pro, Max, Team, and Enterprise subscriptions
- Linux and macOS
- Claude Code 2.1.226 exactly
- New interactive, headless, simultaneous, and background processes

The fork does not switch the graphical Claude Code extension for VS Code.
It does not manage Anthropic Console API profiles.

## How account selection works

The active profile controls each new Claude Code process.
An existing process keeps the profile that it had at startup.

This design lets two sessions use different subscriptions at the same time:

1. Select the first profile.
2. Start the first Claude Code process.
3. Select the second profile.
4. Start the second Claude Code process.

The first process keeps the first account.
The second process uses the second account.

## Build and install

Build the fork from reviewed source.
The project does not publish unsigned binaries.

```bash
git clone https://github.com/CopyPasta-Labs/claude-account.git
cd claude-account
cargo build --locked --release
./target/release/claude-account install
```

You can also give the exact official Claude Code path:

```bash
./target/release/claude-account install --real /absolute/path/to/claude
```

The installer prints the shim directory.
Put that directory before the official Claude Code directory in `PATH`.

Example for the default macOS location:

```bash
export PATH="$HOME/Library/Application Support/claude-account/bin:$PATH"
```

Example for the default Linux location:

```bash
export PATH="$HOME/.local/share/claude-account/bin:$PATH"
```

Open a new terminal after you change the shell startup file.
Then verify command order:

```bash
type -a claude
```

The manager shim must appear before the official executable.

## Register the existing default subscription

Use the current standard Claude Code login as the default profile:

```bash
claude account adopt-default main --email main@example.com
```

This command does not start a new login.
It checks the current default subscription before it registers the profile.

Only one profile can use the default location.

## Add an isolated subscription

Add the second subscription:

```bash
claude account add second --email second@example.com
```

Use `--sso` when the account requires SSO:

```bash
claude account add company --email user@example.com --sso
```

Official Claude Code opens its OAuth page.
Select the account that matches the required email.

The manager checks the email after login.
It does not register a profile when the browser selects the wrong account.

The first OAuth approval can require interaction.
Later switches require no OAuth interaction while both sessions remain valid.

## Daily commands

List all profiles:

```bash
claude account list
```

Show only the active profile name:

```bash
claude account current
```

Select a profile:

```bash
claude account use main
claude account use second
```

Start Claude Code normally after the selection:

```bash
claude
claude -p "Summarize this repository"
claude --model sonnet
```

All non-account arguments pass to the official Claude Code executable.

## Repair an expired or incorrect login

Start the official login flow again:

```bash
claude account reauth NAME
claude account reauth NAME --sso
```

The manager uses the stored email and checks the result.

These official recovery commands bypass the normal identity preflight:

```bash
claude auth login
claude auth logout
claude auth status --json
```

The next normal launch must pass the full identity check.

## Remove a profile

Log out and unregister an inactive profile:

```bash
claude account remove NAME
```

Select another profile before you remove the active profile.
Use `--force` only when you want no active profile.

```bash
claude account remove NAME --force
```

The command keeps settings, sessions, plugins, and history.
The fork rejects `remove --purge` because deletion is not transactional.

## Identity checks

Each normal launch must report these values:

```text
loggedIn = true
authMethod = claude.ai
apiProvider = firstParty
email = the registered profile email
subscriptionType = pro, max, team, or enterprise
```

The manager also reads `oauthAccount.emailAddress` from Claude's account metadata.
This email must match the status output and the registered email.

The preflight uses these launch options when they are present:

- `--settings`
- `--setting-sources`
- `--safe-mode`

The manager rejects `--bare` because that option disables subscription OAuth.

## Profile locations

The default profile uses:

```text
~/.claude/
~/.claude.json
~/.claude/.anthropic/
```

An isolated profile receives:

```text
CLAUDE_CONFIG_DIR=<profile directory>
CLAUDE_SECURESTORAGE_CONFIG_DIR=<profile directory>
ANTHROPIC_CONFIG_DIR=<profile directory>/.anthropic
```

The manager uses this default macOS root:

```text
~/Library/Application Support/claude-account/
```

The manager uses these default Linux roots:

```text
~/.config/claude-account/
~/.local/share/claude-account/
```

`CLAUDE_ACCOUNT_HOME` can select one absolute custom root.
`XDG_CONFIG_HOME` and `XDG_DATA_HOME` can change the standard manager roots.

## Common results

| Result | Meaning | Action |
| --- | --- | --- |
| `No profile is active` | The manager has no selected profile. | Add, adopt, or select a profile. |
| `profile ... does not exist` | The profile name is not registered. | Run `claude account list`. |
| Claude reports another email | OAuth selected the wrong browser account. | Repeat login with the required account. |
| Claude reports `loggedIn: false` | The OAuth session expired or was removed. | Run `claude account reauth NAME`. |
| Version rejected | Claude is not 2.1.226. | Use the audited version. |
| Official Claude starts | Its directory is first in `PATH`. | Put the shim first. |

## Safety limits

The manager checks identity only at process startup.
Claude can reload settings and managed policy after startup.

The manager accepts only Claude Code 2.1.226.
Review the new executable before you change this accepted version.

The state file contains account emails but no OAuth tokens.
Protect the manager directory as private local data.

## Direct access and rollback

Keep the official Claude Code executable installed.
The manager does not replace it.

Run the official executable by its absolute path when you must bypass the manager.
You can also put the official directory first in `PATH` for a temporary rollback.

Restore the shim directory to the first position when you want profile selection again.
