# Complete fork changes and reasons

## Comparison baseline

This document compares the fork with upstream `v0.2.0`.
The exact upstream commit is `7553e3dc083cde4abc99f56258c08a16559701b6`.

The fork changes the package version from `0.2.0` to `0.3.0`.
The fork targets verified Claude Code subscription switching.

## Design change

| Area | Upstream `v0.2.0` | This fork | Reason |
| --- | --- | --- | --- |
| Profile locations | All profiles were isolated. | One default plus isolated profiles. | Keep the standard login. |
| Authentication types | Subscription and Console options existed. | Only subscriptions remain. | Keep OAuth in official Claude. |
| Email | The email was optional. | Each profile requires one email. | Detect a wrong browser account. |
| Identity check | Add checked only `loggedIn`. | Each launch checks status and email. | Prevent silent identity changes. |
| Claude version | macOS required 2.1.144 or later. | All systems require 2.1.226. | Use the reviewed executable. |
| Environment filter | Three variables and a bypass existed. | The full reviewed set is mandatory. | Prevent inherited overrides. |
| Deletion | `remove --purge` deleted data. | The fork rejects purge. | Wait for transactional deletion. |
| Releases | Tags published unsigned archives. | The release workflow is removed. | Do not distribute unsigned binaries. |

## Profile and state changes

### State version 2

The state format changes from version 1 to version 2.

Each profile now contains:

```text
email
location
created_at
```

The location is one of these values:

```text
default
isolated { config_dir }
```

Upstream stored only an isolated `config_dir` and creation time.
The new location type supports the standard Claude Code account.

### Default profile

The fork adds `ProfileLocation::Default`.
This location uses `~/.claude` and `~/.claude.json`.

The manager unsets these variables for the default profile:

```text
CLAUDE_CONFIG_DIR
CLAUDE_SECURESTORAGE_CONFIG_DIR
```

It sets this location:

```text
ANTHROPIC_CONFIG_DIR=~/.claude/.anthropic
```

This behavior keeps the normal Claude Code config and Keychain identity.

### Isolated profile

The fork sets all three profile directories for isolated profiles:

```text
CLAUDE_CONFIG_DIR=<profile directory>
CLAUDE_SECURESTORAGE_CONFIG_DIR=<profile directory>
ANTHROPIC_CONFIG_DIR=<profile directory>/.anthropic
```

Upstream set `CLAUDE_SECURESTORAGE_CONFIG_DIR` only on macOS.
The fork uses the same fixed directory on every supported system.

### Migration

The loader migrates version 1 subscription profiles to version 2.
It gets the expected email from `oauthAccount.emailAddress`.

The migration stops when the email is missing.
It also rejects an imported API profile without reading its API key.

### Loaded-state validation

The loader now validates all stored state before use.
It checks:

- The active profile exists.
- The real Claude path is absolute.
- Each profile name follows the CLI rules.
- Each email has one `@` and no control characters.
- Only one profile uses the default location.
- Each isolated path equals its manager-owned profile path.
- macOS profile names have no case-insensitive collision.
- The state version is supported.

These checks make corrupted or edited state fail closed.
They also stop a stored profile path from targeting another directory.

## CLI changes

### `add`

The `--email` option is now required.
The `--sso` option remains available.

The fork removes the `--console` option.
This removal stops the manager from creating API-billed profiles.

The command now performs this sequence:

1. Reserve the profile name.
2. Validate the name and expected email.
3. Resolve one canonical Claude executable.
4. Check Claude Code version 2.1.226.
5. Create the private isolated directory.
6. Run official `claude auth login`.
7. Verify the full subscription identity.
8. Complete local onboarding for the new unregistered profile.
9. Merge the new profile into current state.

The state merge keeps profile selections that occur during login.

### `adopt-default`

The fork adds this command:

```bash
claude account adopt-default NAME --email EMAIL
```

The command checks and registers the existing standard Claude Code login.
It does not start another OAuth login.

The command permits only one default profile.

### `reauth`

The fork adds these commands:

```bash
claude account reauth NAME
claude account reauth NAME --sso
```

The command runs official login for the stored email.
It then repeats the full identity check.

The reauthentication path does not rewrite onboarding metadata.
This avoids a concurrent read-modify-write on an existing profile.

### `use`, `list`, and `current`

The commands retain their upstream purpose.
`use` now shares the profile reservation with login, logout, and launch operations.

The empty-list help text now includes the required `--email` option.

### Recovery commands

These official auth commands can run when normal preflight fails:

```bash
claude auth login
claude auth logout
claude auth status
```

The recovery detector accepts supported global settings options before `auth`.
It handles `--settings`, `--setting-sources`, and `--safe-mode`.

### `remove`

The command still runs official logout and unregisters the profile.
It keeps non-credential profile data.

The fork rejects `remove --purge` before any profile change.
This restriction remains until deletion can use a transaction.

### `install`

Installation now holds the state lock for the complete operation.
Concurrent and repeated installations produce the same result.

The installer rejects a real-Claude path that resolves to:

- The current manager process
- The installed manager executable
- The managed `claude` shim

The installer resolves and stores one canonical real-Claude path.
It does not save a mutable launcher symlink.

The installer syncs the executable and its parent directory.
It refuses to replace an unexpected file or symlink.

Upstream printed an executable `export PATH` command.
The fork prints only the shim directory.
This change removes shell-injection risk from a custom path.

## Launch and identity changes

### Preflight before each normal launch

Upstream forwarded a normal command without an identity check.
The fork runs `claude auth status --json` before each normal launch.

It requires:

```text
loggedIn = true
authMethod = claude.ai
apiProvider = firstParty
email = the registered email
subscriptionType = pro, max, team, or enterprise
```

The fork also reads `oauthAccount.emailAddress` from Claude's account metadata.
This value must match both email values.

The check rejects:

- A logged-out account
- An API key helper
- Anthropic Console authentication
- Bedrock, Vertex, Foundry, Mantle, and gateway providers
- A free account
- A wrong subscription email
- Missing or malformed local account metadata

### Launch settings

Claude can load authentication settings from command options.
The fork applies these options to the preflight command:

```text
--settings
--setting-sources
--safe-mode
```

The real launch and its preflight therefore see the same authentication settings.

The fork rejects `--bare`.
That option disables subscription OAuth and Keychain access.

### Launch serialization

A normal launch reserves the active profile before preflight.
It keeps the reservation through process replacement.

The state lock also stays active through this boundary.
This sequence prevents login, logout, or selection changes during preflight.

The launch retries when the active profile changes before it gets both locks.

### Executable pinning

Each operation resolves the real Claude path once.
All checks and subprocesses use that canonical path.

This change prevents a launcher symlink from changing targets after validation.
Normal launches also repeat recursion checks against the manager and shim.

### Version probe

All supported systems now query `claude --version`.
Only the exact stable version `2.1.226` passes.

The parser rejects prerelease versions.
It permits build metadata on the audited numeric version.

The probe uses a separate process group and a five-second limit.
The timeout kills and reaps the full probe process group.

Upstream applied a two-second probe only on macOS.
Linux had no version check or timeout.

## Authentication environment changes

Upstream removed only these variables:

```text
ANTHROPIC_API_KEY
ANTHROPIC_AUTH_TOKEN
CLAUDE_CODE_OAUTH_TOKEN
```

Upstream also allowed `CLAUDE_ACCOUNT_PRESERVE_AUTH_ENV=1` to disable the filter.
The fork removes that bypass.

The fork always removes the following complete reviewed set.
The second group comes from Claude Code 2.1.226 behavior.

### Documented authentication and provider variables

```text
ANTHROPIC_API_KEY
ANTHROPIC_AUTH_TOKEN
ANTHROPIC_SCOPE
CLAUDE_CODE_OAUTH_TOKEN
CLAUDE_CODE_OAUTH_REFRESH_TOKEN
CLAUDE_CODE_OAUTH_SCOPES
ANTHROPIC_PROFILE
ANTHROPIC_FEDERATION_RULE_ID
ANTHROPIC_ORGANIZATION_ID
ANTHROPIC_SERVICE_ACCOUNT_ID
ANTHROPIC_WORKSPACE_ID
ANTHROPIC_IDENTITY_TOKEN
ANTHROPIC_IDENTITY_TOKEN_FILE
CLAUDE_CODE_USE_ANTHROPIC_AWS
CLAUDE_CODE_USE_BEDROCK
CLAUDE_CODE_USE_FOUNDRY
CLAUDE_CODE_USE_MANTLE
CLAUDE_CODE_USE_VERTEX
CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST
ANTHROPIC_BASE_URL
ANTHROPIC_CUSTOM_HEADERS
ANTHROPIC_AWS_API_KEY
AWS_BEARER_TOKEN_BEDROCK
ANTHROPIC_AWS_BASE_URL
ANTHROPIC_AWS_WORKSPACE_ID
ANTHROPIC_BEDROCK_BASE_URL
ANTHROPIC_BEDROCK_MANTLE_BASE_URL
ANTHROPIC_FOUNDRY_API_KEY
ANTHROPIC_FOUNDRY_AUTH_TOKEN
ANTHROPIC_FOUNDRY_BASE_URL
ANTHROPIC_FOUNDRY_RESOURCE
ANTHROPIC_VERTEX_BASE_URL
ANTHROPIC_VERTEX_PROJECT_ID
CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH
CLAUDE_CODE_SKIP_BEDROCK_AUTH
CLAUDE_CODE_SKIP_FOUNDRY_AUTH
CLAUDE_CODE_SKIP_MANTLE_AUTH
CLAUDE_CODE_SKIP_VERTEX_AUTH
```

### Claude Code 2.1.226 credential and selector variables

```text
CLAUDE_CODE_USE_GATEWAY
CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD
ANTHROPIC_GOOGLE_CLOUD_BASE_URL
ANTHROPIC_GOOGLE_CLOUD_LOCATION
ANTHROPIC_GOOGLE_CLOUD_PROJECT
ANTHROPIC_GOOGLE_CLOUD_WORKSPACE_ID
CLAUDE_CODE_SKIP_ANTHROPIC_GOOGLE_CLOUD_AUTH
ANTHROPIC_UNIX_SOCKET
CLAUDE_CODE_API_BASE_URL
CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR
CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR
CLAUDE_CODE_WEBSOCKET_AUTH_FILE_DESCRIPTOR
CCR_OAUTH_TOKEN_FILE
CLAUDE_CODE_HOST_CREDS_FILE
CLAUDE_CODE_HOST_AUTH_ENV_VAR
CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH
CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH
CLAUDE_CODE_CUSTOM_OAUTH_URL
CLAUDE_CODE_OAUTH_CLIENT_ID
CLAUDE_LOCAL_OAUTH_API_BASE
CLAUDE_LOCAL_OAUTH_APPS_BASE
CLAUDE_LOCAL_OAUTH_CONSOLE_BASE
USE_LOCAL_OAUTH
USE_STAGING_OAUTH
CLAUDE_BG_AUTH_SNAPSHOT_PATH
CLAUDE_BG_CLAIM_AUTH
CLAUDE_BG_PTY_AUTH
CLAUDE_BG_RV_AUTH
CLAUDE_BG_SOCKET_TOKENS_PATH
CLAUDE_CODE_SESSION_ACCESS_TOKEN
CLAUDE_SESSION_INGRESS_TOKEN_FILE
CLAUDE_TRUSTED_DEVICE_TOKEN
CLAUDE_CODE_ARTIFACTS_API_TOKEN
CLAUDE_BRIDGE_OAUTH_TOKEN
CLAUDE_CODE_HFI_BEARER_TOKEN
AGENT_PROXY_AUTH_TOKEN
ENVIRONMENT_SERVICE_KEY
CLAUDE_CODE_ACCOUNT_UUID
CLAUDE_CODE_ORGANIZATION_UUID
CLAUDE_CODE_USER_EMAIL
CLAUDE_CODE_SUBSCRIPTION_TYPE
CLAUDE_CODE_RATE_LIMIT_TIER
CLAUDE_CODE_ENVIRONMENT_KIND
CLAUDE_CODE_REMOTE_SESSION_ID
CLAUDE_CODE_REMOTE_SESSION_ORIGIN
CLAUDE_CODE_REMOTE
CLAUDE_CODE_SESSION_KIND
CLAUDE_CODE_ACCOUNT_TAGGED_ID
CLAUDE_CODE_DESIGN_OAUTH_CLIENT_ID
_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL
CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL
CLAUDE_BRIDGE_BASE_URL
CLAUDE_BRIDGE_SESSION_INGRESS_URL
CLAUDE_CODE_ARTIFACTS_API_BASE_URL
CLAUDE_CODE_ARTIFACT_ASSET_BASE_URL
CLAUDE_CODE_ARTIFACT_LIVE_BASE_URL
CLAUDE_CODE_GB_BASE_URL
CLAUDE_RUNNER_API_BASE_URL
CLAUDE_REMOTE_TOOLS_BRIDGE_URL
AGENT_PROXY_URL
```

The fork preserves unrelated shell, proxy, certificate, cloud-tool, and child-session variables.
Provider selectors stay removed, and the provider preflight uses the same environment.

## Filesystem and concurrency changes

### Private directories

The directory creator now walks each path component.
It rejects `..`, non-directory components, and unexpected symlink components.

macOS system aliases for `/tmp`, `/var`, and `/etc` remain allowed.
The final managed directory cannot be a symlink.

The creator handles concurrent `AlreadyExists` results.
It then checks that the new component is a real directory.

Managed profile and reservation directories use mode `0700`.

### State and lock files

State, state-lock, and reservation files use mode `0600`.
The program resets unsafe existing modes when it opens these files.

State and lock opens use `O_NOFOLLOW`.
This flag closes the check-to-open symlink race.

### Atomic state writes

The program writes state to a unique temporary file.
It syncs the temporary file before the atomic rename.

It sets mode `0600` on the active state file.
It then syncs the parent directory.

The program removes its temporary file after a failed write.

### Profile reservations

Add, use, reauthenticate, remove, and launch operations reserve the profile name.
macOS reservations use a lowercase key for case-insensitive names.

The reservation prevents two operations from changing one profile concurrently.

### Concurrent state changes

Add and adopt release the state lock during official OAuth work.
They reload state before their final write.

This design keeps unrelated changes that occur during a long login.
It also checks for a duplicate profile after login.

Remove reloads state after official logout.
It cannot restore a profile that another serialized operation removed.

## Path changes

`AppPaths` now stores the standard Claude directory and account metadata path.
Path discovery gets an absolute `HOME` before it handles custom roots.

`CLAUDE_ACCOUNT_HOME` still selects one manager root.
It does not change the standard default Claude account location.

The existing Linux and macOS manager layouts remain compatible.
The macOS ambiguous-layout error now uses shorter recovery instructions.

## Test changes

The macOS suite contains 40 unit tests and 13 end-to-end tests.
Linux runs 38 unit tests and 11 end-to-end tests after macOS-only exclusions.
All tests use fake credentials and temporary directories.

New or expanded tests cover:

- Required email and safe email syntax
- Default and isolated profile locations
- Exact environment removal
- Preserved unrelated environment classes
- Every required identity status field
- Local account email mismatches
- Missing and malformed account metadata
- API-key helper rejection
- Authentication settings in the preflight
- `--bare` rejection
- Version 1 migration
- Imported API-profile rejection
- Invalid state metadata and managed paths
- Multiple default profiles
- File modes and symlink rejection
- Symlink components and reservation symlinks
- Concurrent first installation
- Repeated installation
- Manager and shim recursion
- A stored executable retargeted to the shim
- Canonical executable pinning
- Concurrent remove and use
- State changes during login
- Legacy macOS case-collision recovery
- Version-probe descendant cleanup
- Non-UTF-8 argument forwarding
- Recovery after identity failure
- Linux-safe executable fixtures

The Linux fixture change uses one script path for each identity case.
This avoids Linux `ETXTBSY` errors from executable replacement.

The review also ran repeated end-to-end and concurrent-install stress tests.
Linux, macOS Apple Silicon, and macOS Intel CI all passed.

## Project and dependency changes

### Package metadata

`Cargo.toml` now reports version `0.3.0`.
The homepage and repository point to `CopyPasta-Labs/claude-account`.

The minimum Rust version changes from `1.85` to `1.85.1`.
The new `rust-toolchain.toml` pins Rust, Rustfmt, and Clippy to `1.85.1`.

The fork adds `libc` as a direct dependency.
The code uses it for `O_NOFOLLOW`, process-group signals, and portable error constants.

`Cargo.lock` now records the direct `libc` dependency.

### GitHub Actions

The CI workflow keeps the three upstream targets:

- Linux x86_64
- macOS Apple Silicon
- macOS Intel

The fork pins `actions/checkout` to a full commit identifier.
It also pins `dtolnay/rust-toolchain` to a full commit identifier.

The fork deletes `.github/workflows/release.yml`.
Tags no longer build and publish unsigned archives.

### Repository links

The issue template now sends private reports to the fork's advisory page.
Changelog comparison links now point to the fork.

The CI badge and package links also point to the fork.

## Documentation changes

### `README.md`

The README now describes subscription-only behavior.
It documents default adoption, isolated login, daily selection, recovery, and removal.

It removes unsigned release download instructions.
It also documents exact version support and the identity preflight.

The storage section now distinguishes default and isolated profile locations.
The environment section describes mandatory authentication filtering.

### `SECURITY.md`

The security policy now states the exact ownership boundary.
Official Claude owns OAuth values and refresh behavior.

The policy documents file modes, path controls, state migration, and local trust limits.
It links to the detailed security review.

### `SECURITY-REVIEW.md`

The fork adds a complete security review for the upstream baseline.
It records the identity requirements, environment list, path controls, and verification scope.

### `CONTRIBUTING.md`

The contributor guide now requires Rust 1.85.1 and Claude Code 2.1.226.
It requires two real accounts before a release that changes authentication behavior.

It also forbids real credentials in tests, logs, issues, and pull requests.

### `CHANGELOG.md`

The changelog records the new profile model, identity checks, and security controls.
It identifies `0.3.0` as the fork version.

## Intentional limits

The fork intentionally keeps these limits:

- It manages only the terminal CLI.
- It supports only subscription OAuth.
- It accepts only Claude Code 2.1.226.
- It does not inspect Keychain items or `.credentials.json`.
- It does not call Anthropic OAuth endpoints.
- It does not refresh tokens.
- It does not delete profile data.
- It does not publish binary releases.
- It checks identity only at process startup.

These limits keep the manager outside Claude's credential lifecycle.
They also make unexpected authentication changes fail closed.

## File-by-file summary

| File | Change | Reason |
| --- | --- | --- |
| `.github/ISSUE_TEMPLATE/config.yml` | Point the private report link to CopyPasta Labs. | Keep reports in the fork. |
| `.github/workflows/ci.yml` | Pin both actions to full commits. | Prevent tag movement in CI dependencies. |
| `.github/workflows/release.yml` | Delete the workflow. | Stop unsigned binary publication. |
| `CHANGELOG.md` | Record version 0.3.0 behavior and fork links. | Keep a release-level change record. |
| `CONTRIBUTING.md` | Pin tools and strengthen credential test rules. | Keep contributions inside the security boundary. |
| `Cargo.lock` | Record `libc` as a direct dependency. | Keep locked builds repeatable. |
| `Cargo.toml` | Change version, Rust version, links, and dependencies. | Identify and build the fork correctly. |
| `docs/FORK-CHANGES.md` | Add this complete delta record. | Explain every fork change and reason. |
| `docs/USAGE.md` | Add a focused operating guide. | Separate daily use from the README. |
| `README.md` | Replace general upstream usage with subscription-only usage. | Match the new commands and safety model. |
| `SECURITY.md` | Define the new credential and local trust boundary. | Give reporters and users accurate rules. |
| `SECURITY-REVIEW.md` | Add the detailed review. | Preserve the audit scope and evidence. |
| `rust-toolchain.toml` | Pin Rust 1.85.1 with Rustfmt and Clippy. | Match local and CI tools. |
| `src/account.rs` | Add commands, email checks, profile locations, and safer install/remove behavior. | Implement the subscription workflow. |
| `src/paths.rs` | Add standard Claude paths and retain manager root compatibility. | Support the default profile. |
| `src/process.rs` | Add preflight, filtering, pinning, timeouts, and launch serialization. | Enforce the selected subscription. |
| `src/state.rs` | Add state version 2, migration, validation, locking, and path protections. | Make state safe and deterministic. |
| `tests/e2e.rs` | Replace and expand lifecycle and regression tests. | Verify the new behavior on supported systems. |
