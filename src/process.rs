use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Output};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::paths::AppPaths;
use crate::state::{self, Profile, ProfileLocation, ProfileReservation, StateLock};

const AUTH_ENVIRONMENT_PREFIXES: &[&[u8]] = &[b"ANTHROPIC_", b"CLAUDE_", b"CCR_", b"AGENT_PROXY_"];
const AUTH_ENVIRONMENT_NAMES: &[&[u8]] = &[
    b"_CLAUDE_CODE_ASSUME_FIRST_PARTY_BASE_URL",
    b"AWS_BEARER_TOKEN_BEDROCK",
    b"ENVIRONMENT_SERVICE_KEY",
    b"USE_LOCAL_OAUTH",
    b"USE_STAGING_OAUTH",
];

const MIN_SUPPORTED_CLAUDE_VERSION: (u64, u64, u64) = (2, 1, 226);
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const VERSION_PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub fn exec_active_profile(paths: &AppPaths, arguments: &[OsString]) -> Result<()> {
    let current_executable = env::current_exe().context("failed to locate this executable")?;
    loop {
        let expected_active = state::load(paths)?.active.context(
            "No profile is active. Run `claude account adopt-default NAME --email EMAIL` or `claude account add NAME --email EMAIL`.",
        )?;
        let _profile_reservation = ProfileReservation::acquire(paths, &expected_active)?;
        let _state_lock = StateLock::acquire(paths)?;
        let mut state = state::load(paths)?;
        if state.active.as_deref() != Some(expected_active.as_str()) {
            continue;
        }

        let profile = state
            .profiles
            .get(&expected_active)
            .cloned()
            .with_context(|| format!("active profile `{expected_active}` does not exist"))?;
        let configured_real_claude = state.real_claude.clone();
        let real_claude = resolve_real_claude(
            configured_real_claude.as_deref(),
            &current_executable,
            paths,
        )?;
        validate_platform_support(&real_claude.executable)?;
        if !is_recovery_auth_command(arguments) {
            let authentication_options = authentication_options(arguments)?;
            validate_profile_identity_with_options(
                &real_claude.executable,
                &profile,
                paths,
                &authentication_options,
            )?;
        }
        if real_claude.should_persist_launcher()
            && configured_real_claude.as_deref() != Some(real_claude.launcher.as_path())
        {
            state.real_claude = Some(real_claude.launcher.clone());
            state::save(paths, &state)?;
        }

        let mut command = managed_command(&real_claude.executable, &profile, paths)?;
        command.args(arguments);
        let error = command.exec();
        return Err(error)
            .with_context(|| format!("failed to execute {}", real_claude.executable.display()));
    }
}

fn authentication_options(arguments: &[OsString]) -> Result<Vec<OsString>> {
    let mut options = Vec::new();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if argument == "--" {
            break;
        }
        if argument == "--bare" {
            bail!(
                "The `--bare` option disables subscription OAuth. Run the official Claude executable directly for this option."
            );
        }
        if matches!(argument, "--settings" | "--setting-sources") {
            options.push(OsString::from(argument));
            let value = arguments
                .next()
                .with_context(|| format!("The `{argument}` option requires a value."))?;
            options.push(value.clone());
        } else if argument.starts_with("--settings=")
            || argument.starts_with("--setting-sources=")
            || argument == "--safe-mode"
        {
            options.push(OsString::from(argument));
        }
    }
    Ok(options)
}

fn is_recovery_auth_command(arguments: &[OsString]) -> bool {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let Some(argument) = argument.to_str() else {
            return false;
        };
        if matches!(argument, "--settings" | "--setting-sources") {
            if arguments.next().is_none() {
                return false;
            }
            continue;
        }
        if argument.starts_with("--settings=")
            || argument.starts_with("--setting-sources=")
            || argument == "--safe-mode"
        {
            continue;
        }
        if argument != "auth" {
            return false;
        }
        return matches!(
            arguments.next().and_then(|value| value.to_str()),
            Some("login" | "logout" | "status")
        );
    }
    false
}

pub fn managed_command(real_claude: &Path, profile: &Profile, paths: &AppPaths) -> Result<Command> {
    let mut command = command_without_auth_environment(real_claude);
    let config_dir = profile.config_dir(paths);
    state::ensure_private_dir(config_dir)?;
    let anthropic_config_dir = config_dir.join(".anthropic");
    state::ensure_private_dir(&anthropic_config_dir)?;

    match &profile.location {
        ProfileLocation::Default => {
            command.env_remove("CLAUDE_CONFIG_DIR");
            command.env_remove("CLAUDE_SECURESTORAGE_CONFIG_DIR");
        }
        ProfileLocation::Isolated { .. } => {
            command.env("CLAUDE_CONFIG_DIR", config_dir);
            command.env("CLAUDE_SECURESTORAGE_CONFIG_DIR", config_dir);
        }
    }
    command.env("ANTHROPIC_CONFIG_DIR", anthropic_config_dir);

    Ok(command)
}

fn command_without_auth_environment(program: &Path) -> Command {
    let mut command = Command::new(program);
    remove_auth_environment(&mut command, env::vars_os().map(|(name, _)| name));
    command
}

fn remove_auth_environment(command: &mut Command, variables: impl IntoIterator<Item = OsString>) {
    for variable in variables {
        if is_auth_environment_variable(&variable) {
            command.env_remove(variable);
        }
    }
}

fn is_auth_environment_variable(variable: &OsStr) -> bool {
    let variable = variable.as_bytes();
    AUTH_ENVIRONMENT_PREFIXES
        .iter()
        .any(|prefix| variable.starts_with(prefix))
        || AUTH_ENVIRONMENT_NAMES.contains(&variable)
}

#[derive(Debug, Deserialize)]
struct AuthStatus {
    #[serde(rename = "loggedIn")]
    logged_in: Option<bool>,
    #[serde(rename = "authMethod")]
    auth_method: Option<String>,
    #[serde(rename = "apiProvider")]
    api_provider: Option<String>,
    email: Option<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

pub fn validate_profile_identity(
    real_claude: &Path,
    profile: &Profile,
    paths: &AppPaths,
) -> Result<()> {
    validate_profile_identity_with_options(real_claude, profile, paths, &[])
}

fn validate_profile_identity_with_options(
    real_claude: &Path,
    profile: &Profile,
    paths: &AppPaths,
    authentication_options: &[OsString],
) -> Result<()> {
    let output = managed_command(real_claude, profile, paths)?
        .args(authentication_options)
        .args(["auth", "status", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("failed to verify the Claude subscription")?;
    if !output.status.success() {
        bail!(
            "Claude did not report a valid login for `{}`",
            profile.email
        );
    }
    let status: AuthStatus = serde_json::from_slice(&output.stdout)
        .context("Claude returned invalid JSON from `auth status --json`")?;
    if status.logged_in != Some(true) {
        bail!("Claude reports that `{}` is logged out", profile.email);
    }
    if status.auth_method.as_deref() != Some("claude.ai") {
        bail!(
            "Claude is not using subscription authentication for `{}`",
            profile.email
        );
    }
    if status.api_provider.as_deref() != Some("firstParty") {
        bail!(
            "Claude is not using the first-party provider for `{}`",
            profile.email
        );
    }
    let status_email = status
        .email
        .as_deref()
        .context("Claude did not report an account email")?;
    if !status_email.eq_ignore_ascii_case(&profile.email) {
        bail!(
            "Claude reported `{status_email}`, but this profile requires `{}`",
            profile.email
        );
    }
    if !matches!(
        status.subscription_type.as_deref(),
        Some("pro" | "max" | "team" | "enterprise")
    ) {
        bail!(
            "Claude did not report a supported subscription for `{}`",
            profile.email
        );
    }

    let claude_json_path = profile.claude_json_path(paths);
    let claude_json: serde_json::Value = serde_json::from_slice(
        &fs::read(&claude_json_path)
            .with_context(|| format!("failed to read {}", claude_json_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", claude_json_path.display()))?;
    let file_email = claude_json
        .get("oauthAccount")
        .and_then(|account| account.get("emailAddress"))
        .and_then(serde_json::Value::as_str)
        .context("Claude account state does not contain oauthAccount.emailAddress")?;
    if !file_email.eq_ignore_ascii_case(&profile.email)
        || !file_email.eq_ignore_ascii_case(status_email)
    {
        bail!(
            "Claude account state contains `{file_email}`, but this profile requires `{}`",
            profile.email
        );
    }
    Ok(())
}

pub fn validate_platform_support(real_claude: &Path) -> Result<()> {
    let output = query_claude_version(real_claude)?;

    if !output.status.success() {
        bail!(
            "Failed to query the Claude Code version from {}. Supported versions are stable Claude Code 2.x releases from 2.1.226.",
            real_claude.display()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reported = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    let version = parse_claude_version(reported).with_context(|| {
        format!(
            "Could not parse Claude Code version `{reported}`. Supported versions are stable Claude Code 2.x releases from 2.1.226."
        )
    })?;
    if !version.is_supported() {
        bail!(
            "Supported versions are stable Claude Code 2.x releases from 2.1.226. Version {version} is installed."
        );
    }

    Ok(())
}

fn query_claude_version(real_claude: &Path) -> Result<Output> {
    let deadline = Instant::now() + VERSION_PROBE_TIMEOUT;
    let mut command = command_without_auth_environment(real_claude);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = command.spawn().with_context(|| {
        format!(
            "failed to query Claude Code version from {}",
            real_claude.display()
        )
    })?;
    let mut probe = VersionProbe::new(child);
    let stdout = probe
        .child
        .stdout
        .take()
        .context("Claude Code version probe did not capture stdout")?;
    let stderr = probe
        .child
        .stderr
        .take()
        .context("Claude Code version probe did not capture stderr")?;
    let mut stdout_reader = Some(read_version_stream(stdout));
    let mut stderr_reader = Some(read_version_stream(stderr));
    let mut status = None;

    loop {
        if Instant::now() >= deadline {
            return timeout_version_probe(&mut probe, real_claude);
        }

        if status.is_none() {
            status = probe.try_wait().with_context(|| {
                format!(
                    "failed while querying Claude Code version from {}",
                    real_claude.display()
                )
            })?;
        }

        let streams_finished = stdout_reader.as_ref().is_some_and(JoinHandle::is_finished)
            && stderr_reader.as_ref().is_some_and(JoinHandle::is_finished);
        if streams_finished {
            if let Some(status) = status.take() {
                let stdout = collect_version_stream(stdout_reader.take().unwrap(), "stdout")?;
                let stderr = collect_version_stream(stderr_reader.take().unwrap(), "stderr")?;
                if Instant::now() >= deadline {
                    return timeout_version_probe(&mut probe, real_claude);
                }
                probe.mark_complete();
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(VERSION_PROBE_POLL_INTERVAL.min(remaining));
    }
}

fn timeout_version_probe(probe: &mut VersionProbe, real_claude: &Path) -> Result<Output> {
    probe.terminate_and_reap().with_context(|| {
        format!(
            "The Claude Code version query timed out for {}. The manager could not stop the version query.",
            real_claude.display()
        )
    })?;
    bail!(
        "The Claude Code version query timed out for {}. Supported versions are stable Claude Code 2.x releases from 2.1.226.",
        real_claude.display()
    )
}

struct VersionProbe {
    child: Child,
    process_group: i32,
    reaped: bool,
    complete: bool,
}

impl VersionProbe {
    fn new(child: Child) -> Self {
        Self {
            process_group: child.id() as i32,
            child,
            reaped: false,
            complete: false,
        }
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }

    fn terminate_and_reap(&mut self) -> Result<()> {
        let terminate_result = terminate_process_group(self.process_group);
        let reap_result = if self.reaped {
            Ok(())
        } else {
            self.child.wait().map(|_| ()).map_err(anyhow::Error::from)
        };
        if reap_result.is_ok() {
            self.reaped = true;
        }
        if terminate_result.is_ok() && reap_result.is_ok() {
            self.complete = true;
        }

        terminate_result.context("failed to terminate Claude Code version probe process group")?;
        reap_result.context("failed to reap Claude Code version probe")?;
        Ok(())
    }

    fn mark_complete(&mut self) {
        debug_assert!(self.reaped);
        self.complete = true;
    }
}

impl Drop for VersionProbe {
    fn drop(&mut self) {
        if self.complete {
            return;
        }

        let _ = terminate_process_group(self.process_group);
        if !self.reaped {
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

fn terminate_process_group(process_group: i32) -> io::Result<()> {
    // A negative pid asks kill(2) to signal every process in this process group.
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn read_version_stream<R>(mut stream: R) -> JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut contents = Vec::new();
        stream.read_to_end(&mut contents)?;
        Ok(contents)
    })
}

fn collect_version_stream(
    reader: JoinHandle<io::Result<Vec<u8>>>,
    stream_name: &str,
) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("Claude Code version probe {stream_name} reader panicked"))?
        .with_context(|| format!("failed to read Claude Code version probe {stream_name}"))
}

#[derive(Debug, Eq, PartialEq)]
struct ClaudeVersion {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Option<String>,
}

impl ClaudeVersion {
    fn is_supported(&self) -> bool {
        let numeric_core = (self.major, self.minor, self.patch);
        self.major == 2 && numeric_core >= MIN_SUPPORTED_CLAUDE_VERSION && self.prerelease.is_none()
    }
}

impl fmt::Display for ClaudeVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(prerelease) = &self.prerelease {
            write!(formatter, "-{prerelease}")?;
        }
        Ok(())
    }
}

fn parse_claude_version(value: &str) -> Option<ClaudeVersion> {
    value.split_whitespace().find_map(|word| {
        let version = word.strip_prefix('v').unwrap_or(word);
        let version = version.split('+').next()?;
        let (numeric_core, prerelease) = match version.split_once('-') {
            Some((_numeric_core, "")) => return None,
            Some((numeric_core, prerelease)) => (numeric_core, Some(prerelease.to_owned())),
            None => (version, None),
        };
        let mut parts = numeric_core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        (parts.next().is_none()).then_some(ClaudeVersion {
            major,
            minor,
            patch,
            prerelease,
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealClaudeResolution {
    pub launcher: PathBuf,
    pub executable: PathBuf,
    persist_launcher: bool,
}

impl RealClaudeResolution {
    pub fn should_persist_launcher(&self) -> bool {
        self.persist_launcher
    }
}

pub fn resolve_real_claude(
    configured: Option<&Path>,
    current_executable: &Path,
    paths: &AppPaths,
) -> Result<RealClaudeResolution> {
    if let Some(explicit) = env::var_os("CLAUDE_ACCOUNT_REAL_CLAUDE") {
        let explicit = PathBuf::from(explicit);
        if !explicit.is_absolute() {
            bail!("CLAUDE_ACCOUNT_REAL_CLAUDE must contain an absolute path");
        }
        let mut resolved = pin_real_claude_candidate(&explicit, current_executable, paths)?;
        resolved.persist_launcher = false;
        return Ok(resolved);
    }

    if let Some(configured) = configured {
        match pin_real_claude_candidate(configured, current_executable, paths) {
            Ok(resolved) => return Ok(resolved),
            Err(error) => {
                if !configured.is_absolute() || !configured_candidate_is_missing(configured) {
                    return Err(error);
                }
            }
        }
    }

    let path = env::var_os("PATH").context("PATH is not set")?;
    let current_directory = env::current_dir().context("failed to locate the current directory")?;
    for directory in env::split_paths(&path) {
        let candidate = if directory.as_os_str().is_empty() {
            current_directory.join("claude")
        } else if directory.is_absolute() {
            directory.join("claude")
        } else {
            current_directory.join(directory).join("claude")
        };
        if candidate == paths.shim {
            continue;
        }
        if let Ok(resolved) = pin_real_claude_candidate(&candidate, current_executable, paths) {
            return Ok(resolved);
        }
    }

    bail!(
        "could not find the real `claude` executable; pass it with \
         `claude-account install --real /path/to/claude`"
    )
}

fn configured_candidate_is_missing(candidate: &Path) -> bool {
    matches!(
        fs::metadata(candidate),
        Err(error) if error.kind() == io::ErrorKind::NotFound
    )
}

pub fn validate_executable(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("Claude executable does not exist: {}", path.display()))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        bail!("path is not executable: {}", path.display());
    }
    Ok(())
}

pub fn pin_real_claude_candidate(
    candidate: &Path,
    current_executable: &Path,
    paths: &AppPaths,
) -> Result<RealClaudeResolution> {
    if !candidate.is_absolute() {
        bail!(
            "candidate must be an absolute path: {}",
            candidate.display()
        );
    }
    validate_executable(candidate)?;
    let candidate_canonical = fs::canonicalize(candidate)
        .with_context(|| format!("failed to resolve {}", candidate.display()))?;
    let current_canonical = fs::canonicalize(current_executable)
        .with_context(|| format!("failed to resolve {}", current_executable.display()))?;
    if candidate_canonical == current_canonical {
        bail!("candidate points back to the claude-account wrapper");
    }
    for managed in [&paths.installed_executable, &paths.shim] {
        if fs::canonicalize(managed).ok().as_ref() == Some(&candidate_canonical) {
            bail!("candidate points back to a managed claude-account path");
        }
    }
    Ok(RealClaudeResolution {
        launcher: candidate.to_path_buf(),
        executable: candidate_canonical,
        persist_launcher: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn command_environment(command: &Command) -> BTreeMap<OsString, Option<OsString>> {
        command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
            .collect()
    }

    #[test]
    fn parses_claude_code_version_output() {
        assert_eq!(
            parse_claude_version("2.1.144 (Claude Code)"),
            Some(ClaudeVersion {
                major: 2,
                minor: 1,
                patch: 144,
                prerelease: None,
            })
        );
        assert_eq!(
            parse_claude_version("v2.2.0-beta.1 (Claude Code)"),
            Some(ClaudeVersion {
                major: 2,
                minor: 2,
                patch: 0,
                prerelease: Some("beta.1".to_owned()),
            })
        );
        assert_eq!(parse_claude_version("Claude Code"), None);
    }

    #[test]
    fn accepts_stable_2x_versions_from_the_supported_floor() {
        let supports = |version| parse_claude_version(version).unwrap().is_supported();

        assert!(!supports("1.99.999"));
        assert!(!supports("2.1.225"));
        assert!(!supports("2.1.226-beta.1"));
        assert!(supports("2.1.226"));
        assert!(supports("2.1.226+build.1"));
        assert!(supports("2.1.237"));
        assert!(supports("2.2.0"));
        assert!(!supports("2.2.0-rc.1"));
        assert!(!supports("3.0.0"));
    }

    #[test]
    fn displays_prerelease_in_version_errors() {
        assert_eq!(
            parse_claude_version("2.1.226-beta.1").unwrap().to_string(),
            "2.1.226-beta.1"
        );
    }

    #[test]
    fn auth_environment_filter_covers_future_namespaces() {
        for variable in [
            "ANTHROPIC_FUTURE_PROVIDER",
            "CLAUDE_FUTURE_TOKEN",
            "CLAUDE_CODE_FUTURE_TOKEN",
            "CCR_FUTURE_TOKEN_FILE",
            "AGENT_PROXY_FUTURE_TOKEN",
            "AWS_BEARER_TOKEN_BEDROCK",
            "USE_LOCAL_OAUTH",
            "USE_STAGING_OAUTH",
            "ENVIRONMENT_SERVICE_KEY",
        ] {
            assert!(
                is_auth_environment_variable(std::ffi::OsStr::new(variable)),
                "{variable} was not classified"
            );
        }

        for variable in [
            "AWS_PROFILE",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "SSL_CERT_FILE",
            "PATH",
            "CLAUDE",
        ] {
            assert!(
                !is_auth_environment_variable(std::ffi::OsStr::new(variable)),
                "{variable} was classified"
            );
        }
    }

    #[test]
    fn command_removes_namespaced_auth_environment() {
        let mut command = Command::new("/usr/bin/env");
        remove_auth_environment(
            &mut command,
            [
                OsString::from("ANTHROPIC_FUTURE_PROVIDER"),
                OsString::from("CLAUDE_CODE_FUTURE_TOKEN"),
                OsString::from("CCR_FUTURE_TOKEN_FILE"),
                OsString::from("AGENT_PROXY_FUTURE_TOKEN"),
                OsString::from("AWS_PROFILE"),
            ],
        );
        let environment = command_environment(&command);

        for variable in [
            "ANTHROPIC_FUTURE_PROVIDER",
            "CLAUDE_CODE_FUTURE_TOKEN",
            "CCR_FUTURE_TOKEN_FILE",
            "AGENT_PROXY_FUTURE_TOKEN",
        ] {
            assert_eq!(
                environment.get(OsString::from(variable).as_os_str()),
                Some(&None),
                "{variable} was not removed"
            );
        }
        assert!(!environment.contains_key(OsStr::new("AWS_PROFILE")));
    }

    #[test]
    fn authentication_options_match_the_actual_launch() {
        let options = authentication_options(&[
            OsString::from("--safe-mode"),
            OsString::from("--settings"),
            OsString::from("settings.json"),
            OsString::from("--setting-sources=user,project"),
            OsString::from("--model"),
            OsString::from("sonnet"),
        ])
        .unwrap();

        assert_eq!(
            options,
            [
                OsString::from("--safe-mode"),
                OsString::from("--settings"),
                OsString::from("settings.json"),
                OsString::from("--setting-sources=user,project"),
            ]
        );
        assert!(authentication_options(&[OsString::from("--bare")]).is_err());
        assert!(authentication_options(&[OsString::from("--settings")]).is_err());
        assert!(authentication_options(&[OsString::from("--"), OsString::from("--bare")]).is_ok());
    }

    #[test]
    fn recovery_commands_accept_supported_global_options() {
        for arguments in [
            vec![OsString::from("auth"), OsString::from("status")],
            vec![
                OsString::from("--settings"),
                OsString::from("settings.json"),
                OsString::from("auth"),
                OsString::from("login"),
            ],
            vec![
                OsString::from("--setting-sources=user"),
                OsString::from("--safe-mode"),
                OsString::from("auth"),
                OsString::from("logout"),
            ],
        ] {
            assert!(is_recovery_auth_command(&arguments));
        }
        assert!(!is_recovery_auth_command(&[
            OsString::from("--bare"),
            OsString::from("auth"),
            OsString::from("status"),
        ]));
        assert!(!is_recovery_auth_command(&[
            OsString::from("--settings"),
            OsString::from("auth"),
        ]));
    }

    #[test]
    fn identity_preflight_receives_authentication_options() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        state::ensure_private_dir(&config_dir).unwrap();
        fs::write(
            config_dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"work@example.com"}}"#,
        )
        .unwrap();
        let profile = Profile::isolated(config_dir, "work@example.com");
        let calls = temp.path().join("calls.log");
        let fake_claude = temp.path().join("claude");
        fs::write(
            &fake_claude,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s\\n' '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"work@example.com\",\"subscriptionType\":\"max\"}}'\n",
                calls.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
        let options = [
            OsString::from("--settings"),
            OsString::from("{}"),
            OsString::from("--safe-mode"),
        ];

        validate_profile_identity_with_options(&fake_claude, &profile, &paths, &options).unwrap();

        assert_eq!(
            fs::read_to_string(calls).unwrap().trim(),
            "--settings {} --safe-mode auth status --json"
        );
    }

    #[test]
    fn launch_settings_cannot_bypass_subscription_preflight() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        state::ensure_private_dir(&config_dir).unwrap();
        fs::write(
            config_dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"work@example.com"}}"#,
        )
        .unwrap();
        let profile = Profile::isolated(config_dir, "work@example.com");
        let fake_claude = temp.path().join("claude");
        fs::write(
            &fake_claude,
            "#!/bin/sh\nmethod='claude.ai'\nif [ \"$1\" = \"--settings\" ]; then method='api_key'; fi\nprintf '{\"loggedIn\":true,\"authMethod\":\"%s\",\"apiProvider\":\"firstParty\",\"email\":\"work@example.com\",\"subscriptionType\":\"max\"}\\n' \"$method\"\n",
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        validate_profile_identity(&fake_claude, &profile, &paths).unwrap();
        let options = [OsString::from("--settings"), OsString::from("{}")];
        let error =
            validate_profile_identity_with_options(&fake_claude, &profile, &paths, &options)
                .unwrap_err();

        assert!(error.to_string().contains("subscription authentication"));
    }

    #[test]
    fn managed_command_preserves_unrelated_environment_classes() {
        let command = command_without_auth_environment(Path::new("/usr/bin/env"));
        let environment = command_environment(&command);
        for variable in [
            "AWS_PROFILE",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
            "NODE_EXTRA_CA_CERTS",
            "SSL_CERT_FILE",
            "ANTHROPIC_BETAS",
            "CLAUDE_CODE_CHILD_SESSION",
        ] {
            assert!(
                !environment.contains_key(OsString::from(variable).as_os_str()),
                "{variable} was changed"
            );
        }
    }

    #[test]
    fn isolated_profile_sets_all_profile_directories() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        let profile = Profile::isolated(config_dir.clone(), "work@example.com");
        let command = managed_command(Path::new("/usr/bin/env"), &profile, &paths).unwrap();
        let environment = command_environment(&command);

        assert_eq!(
            environment.get(OsString::from("CLAUDE_CONFIG_DIR").as_os_str()),
            Some(&Some(config_dir.clone().into_os_string()))
        );
        assert_eq!(
            environment.get(OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR").as_os_str()),
            Some(&Some(config_dir.clone().into_os_string()))
        );
        assert_eq!(
            environment.get(OsString::from("ANTHROPIC_CONFIG_DIR").as_os_str()),
            Some(&Some(config_dir.join(".anthropic").into_os_string()))
        );
    }

    #[test]
    fn default_profile_unsets_claude_selectors() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let profile = Profile::default("work@example.com");
        let command = managed_command(Path::new("/usr/bin/env"), &profile, &paths).unwrap();
        let environment = command_environment(&command);

        assert_eq!(
            environment.get(OsString::from("CLAUDE_CONFIG_DIR").as_os_str()),
            Some(&None)
        );
        assert_eq!(
            environment.get(OsString::from("CLAUDE_SECURESTORAGE_CONFIG_DIR").as_os_str()),
            Some(&None)
        );
        assert_eq!(
            environment.get(OsString::from("ANTHROPIC_CONFIG_DIR").as_os_str()),
            Some(&Some(
                paths.default_claude_dir.join(".anthropic").into_os_string()
            ))
        );
    }

    #[test]
    fn identity_validation_rejects_each_invalid_status_field() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        state::ensure_private_dir(&config_dir).unwrap();
        fs::write(
            config_dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"work@example.com"}}"#,
        )
        .unwrap();
        let profile = Profile::isolated(config_dir, "work@example.com");
        let cases = [
            (
                r#"{"loggedIn":false,"authMethod":"claude.ai","apiProvider":"firstParty","email":"work@example.com","subscriptionType":"max"}"#,
                "logged out",
            ),
            (
                r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty","email":"work@example.com","subscriptionType":"max"}"#,
                "subscription authentication",
            ),
            (
                r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"bedrock","email":"work@example.com","subscriptionType":"max"}"#,
                "first-party provider",
            ),
            (
                r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"other@example.com","subscriptionType":"max"}"#,
                "requires `work@example.com`",
            ),
            (
                r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"work@example.com","subscriptionType":"free"}"#,
                "supported subscription",
            ),
        ];

        for (index, (json, expected)) in cases.into_iter().enumerate() {
            let fake_claude = temp.path().join(format!("claude-{index}"));
            fs::write(
                &fake_claude,
                format!("#!/bin/sh\nprintf '%s\\n' '{json}'\n"),
            )
            .unwrap();
            fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

            let error = validate_profile_identity(&fake_claude, &profile, &paths).unwrap_err();

            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn identity_validation_rejects_a_claude_json_email_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        state::ensure_private_dir(&config_dir).unwrap();
        fs::write(
            config_dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"other@example.com"}}"#,
        )
        .unwrap();
        let profile = Profile::isolated(config_dir, "work@example.com");
        let fake_claude = temp.path().join("claude");
        fs::write(
            &fake_claude,
            "#!/bin/sh\nprintf '%s\\n' '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"work@example.com\",\"subscriptionType\":\"max\"}'\n",
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        let error = validate_profile_identity(&fake_claude, &profile, &paths).unwrap_err();

        assert!(error.to_string().contains("other@example.com"));
    }

    #[test]
    fn identity_validation_rejects_missing_and_malformed_account_state() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        state::ensure_private_dir(&config_dir).unwrap();
        let profile = Profile::isolated(config_dir.clone(), "work@example.com");
        let fake_claude = temp.path().join("claude");
        fs::write(
            &fake_claude,
            "#!/bin/sh\nprintf '%s\\n' '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"work@example.com\",\"subscriptionType\":\"max\"}'\n",
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        let missing = validate_profile_identity(&fake_claude, &profile, &paths).unwrap_err();
        assert!(format!("{missing:#}").contains("failed to read"));

        fs::write(config_dir.join(".claude.json"), b"not json\n").unwrap();
        let malformed = validate_profile_identity(&fake_claude, &profile, &paths).unwrap_err();
        assert!(format!("{malformed:#}").contains("failed to parse"));

        fs::write(config_dir.join(".claude.json"), b"{}\n").unwrap();
        let incomplete = validate_profile_identity(&fake_claude, &profile, &paths).unwrap_err();
        assert!(format!("{incomplete:#}").contains("oauthAccount.emailAddress"));
    }

    #[test]
    fn profile_api_key_helper_cannot_pass_subscription_preflight() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let config_dir = paths.profile_dir("work");
        state::ensure_private_dir(config_dir.join("settings.json").parent().unwrap()).unwrap();
        fs::write(
            config_dir.join("settings.json"),
            r#"{"apiKeyHelper":"/usr/local/bin/custom-key"}"#,
        )
        .unwrap();
        fs::write(
            config_dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"work@example.com"}}"#,
        )
        .unwrap();
        let profile = Profile::isolated(config_dir, "work@example.com");
        let fake_claude = temp.path().join("claude");
        fs::write(
            &fake_claude,
            "#!/bin/sh\nprintf '%s\\n' '{\"loggedIn\":true,\"authMethod\":\"api_key\",\"apiProvider\":\"firstParty\",\"email\":\"work@example.com\",\"subscriptionType\":\"max\"}'\n",
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        let error = validate_profile_identity(&fake_claude, &profile, &paths).unwrap_err();

        assert!(error.to_string().contains("subscription authentication"));
    }
}
