# Security review

This review covers upstream commit `7553e3dc083cde4abc99f56258c08a16559701b6`.
The review examined authentication selection, process environments, state writes, path handling, installation, and profile removal.

The review did not inspect or record real credentials.
Tests use placeholder emails, fake status data, and temporary directories.

## Security boundary

The official Claude Code executable owns OAuth login, secure storage, token refresh, and logout.
This manager selects a Claude config location and verifies the resulting identity.
It does not call Anthropic authentication endpoints.
It does not read Keychain items or `.credentials.json`.

The manager reads only `oauthAccount.emailAddress` from Claude's local JSON account file.
It compares that value with the status output and the registered email.

## Required identity

Each normal launch must pass these checks:

```text
loggedIn = true
authMethod = claude.ai
apiProvider = firstParty
email = registered email
subscriptionType = pro, max, team, or enterprise
```

The local `oauthAccount.emailAddress` value must match both email values.
An API-key helper fails this check when Claude reports a non-subscription authentication method.

The preflight uses the launch values for `--settings`, `--setting-sources`, and `--safe-mode`.
The manager rejects `--bare` because it disables subscription OAuth and Keychain access.

## Removed process variables

The manager removes these documented authentication and provider variables:

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

The manager also removes these variables found in Claude Code 2.1.226:

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

The second list is version-specific compatibility data.
The manager accepts only Claude Code 2.1.226.
Review each new Claude Code release before changing the accepted version.

The manager preserves general shell, proxy, certificate, cloud-tool, and session variables.
Examples include `HTTP_PROXY`, `AWS_PROFILE`, `SSL_CERT_FILE`, and `CLAUDE_CODE_CHILD_SESSION`.

No environment variable can disable the authentication filter.

## Profile locations

The default profile unsets both Claude config selectors.
It sets `ANTHROPIC_CONFIG_DIR` to `~/.claude/.anthropic`.
This behavior preserves Claude Code's default Keychain entry.

An isolated profile sets these locations:

```text
CLAUDE_CONFIG_DIR=<absolute profile directory>
CLAUDE_SECURESTORAGE_CONFIG_DIR=<same absolute profile directory>
ANTHROPIC_CONFIG_DIR=<profile directory>/.anthropic
```

The manager creates profile directories with mode `0700`.
It rejects a managed path that contains a symlink component.

## State and installation

The manager writes state with an atomic rename.
It syncs the file and its parent directory.
It resets state and lock file modes to `0600`.
File opens use `O_NOFOLLOW` for state and lock files.

State loading validates profile names, emails, locations, and the active profile.
Each isolated path must match its manager-owned profile directory.
Only one profile can use the default Claude location.

Installation uses the state lock for the full operation.
Repeated and concurrent installations produce the same managed executable and shim.
The installer rejects manager and shim paths as the real Claude executable.
Normal launches repeat this executable recursion check.
Each operation resolves one canonical executable path before it checks or runs Claude.
The version probe has a five-second limit on each supported system.

Normal launches reserve the active profile through preflight and process replacement.
This reservation prevents a concurrent logout or login change during preflight.

The manager rejects `remove --purge`.
This restriction prevents partial deletion before a transactional design exists.

GitHub Actions use full commit identifiers.
This fork does not publish unsigned binary releases.
The repository pins Rust 1.85.1 in `rust-toolchain.toml`.

## Verification

Automated tests cover:

- every removed authentication variable
- preserved environment classes
- default and isolated profile locations
- each required status field
- email mismatches and malformed account files
- state version migration
- symlink rejection and file permissions
- concurrent and repeated installation
- recursion prevention
- non-UTF-8 argument forwarding
- recovery after an identity failure

Release validation must also alternate two real subscriptions.
Do not put real credentials or account files in test output.

## Runtime limit

Claude can reload settings, policy helpers, and server-managed policy during a session.
The manager verifies the selected identity before process start.
It cannot verify a policy change that occurs after Claude starts.
