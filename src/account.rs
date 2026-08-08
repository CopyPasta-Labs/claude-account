use std::env;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{Map, Value};

use crate::paths::AppPaths;
use crate::process;
use crate::state::{self, Profile, ProfileLocation, ProfileReservation, StateLock};

#[derive(Debug, Parser)]
#[command(
    name = "claude account",
    version,
    about = "Switch verified Claude Code subscription profiles"
)]
pub struct AccountCli {
    #[command(subcommand)]
    command: AccountCommand,
}

#[derive(Debug, Subcommand)]
enum AccountCommand {
    /// Create a profile and open Claude Code's normal login flow
    Add {
        /// Profile name, such as work or personal
        name: String,
        /// Pre-fill the email address in Claude's login flow
        #[arg(long)]
        email: String,
        /// Force SSO authentication
        #[arg(long)]
        sso: bool,
    },
    /// Register the existing default Claude Code subscription
    AdoptDefault {
        /// Profile name, such as work or personal
        name: String,
        /// Expected subscription email address
        #[arg(long)]
        email: String,
    },
    /// Run Claude Code's login flow again for a registered profile
    Reauth {
        name: String,
        /// Force SSO authentication
        #[arg(long)]
        sso: bool,
    },
    /// Select the profile used by future Claude processes
    Use { name: String },
    /// List registered profiles
    List,
    /// Print only the active profile name
    Current,
    /// Safely unregister one exact name from legacy case-colliding state
    ResolveCaseCollision {
        /// Exact profile name to unregister
        name: String,
    },
    /// Log out and unregister a profile
    Remove {
        name: String,
        /// Also delete settings, sessions, plugins, and history
        #[arg(long, requires = "yes")]
        purge: bool,
        /// Confirm permanent deletion with --purge
        #[arg(long)]
        yes: bool,
        /// Allow removing the active profile
        #[arg(long)]
        force: bool,
    },
    /// Install the transparent `claude` shim
    Install {
        /// Absolute path to the real Claude Code executable
        #[arg(long)]
        real: Option<PathBuf>,
    },
}

impl AccountCli {
    pub fn run(self, paths: &AppPaths) -> Result<()> {
        match self.command {
            AccountCommand::Add { name, email, sso } => add(paths, &name, &email, sso),
            AccountCommand::AdoptDefault { name, email } => adopt_default(paths, &name, &email),
            AccountCommand::Reauth { name, sso } => reauth(paths, &name, sso),
            AccountCommand::Use { name } => use_profile(paths, &name),
            AccountCommand::List => list(paths),
            AccountCommand::Current => current(paths),
            AccountCommand::ResolveCaseCollision { name } => resolve_case_collision(paths, &name),
            AccountCommand::Remove {
                name,
                purge,
                yes: _,
                force,
            } => remove(paths, &name, purge, force),
            AccountCommand::Install { real } => install(paths, real.as_deref()),
        }
    }
}

fn add(paths: &AppPaths, name: &str, email: &str, sso: bool) -> Result<()> {
    validate_profile_name(name)?;
    validate_expected_email(email)?;
    let _profile_reservation = ProfileReservation::acquire(paths, name)?;
    let current_executable = env::current_exe().context("failed to locate this executable")?;
    let initial_real_claude = {
        let _state_lock = StateLock::acquire(paths)?;
        let state = state::load(paths)?;
        if state.profiles.contains_key(name) {
            bail!("profile `{name}` already exists");
        }
        validate_new_profile_name(&state, name)?;
        state.real_claude.clone()
    };

    let real_claude =
        process::resolve_real_claude(initial_real_claude.as_deref(), &current_executable, paths)?;
    process::validate_platform_support(&real_claude)?;
    let profile_dir = paths.profile_dir(name);
    state::ensure_private_dir(&profile_dir)?;
    let profile = Profile::isolated(profile_dir, email);

    println!("Logging in profile `{name}` using Claude Code...");
    let mut login = process::managed_command(&real_claude, &profile, paths)?;
    login.args(["auth", "login", "--email", email]);
    if sso {
        login.arg("--sso");
    }
    let login_status = login.status().context("failed to start Claude login")?;
    if !login_status.success() {
        bail!(
            "Claude login failed for `{name}`. The profile directory remains available for a retry."
        );
    }

    process::validate_profile_identity(&real_claude, &profile, paths)
        .with_context(|| format!("login verification failed for profile `{name}`"))?;

    complete_claude_onboarding(&profile.claude_json_path(paths))?;

    let first_profile;
    {
        let _state_lock = StateLock::acquire(paths)?;
        let mut state = state::load(paths)?;
        if state.profiles.contains_key(name) {
            bail!("profile `{name}` was added by another process");
        }
        validate_new_profile_name(&state, name)?;

        first_profile = state.profiles.is_empty();
        if state.real_claude == initial_real_claude {
            state.real_claude = Some(real_claude);
        }
        state.profiles.insert(name.to_owned(), profile);
        if first_profile {
            state.active = Some(name.to_owned());
        }
        state::save(paths, &state)?;
    }

    if first_profile {
        println!("Added `{name}` and made it active.");
    } else {
        println!("Added `{name}`. Activate it with `claude account use {name}`.");
    }
    Ok(())
}

fn adopt_default(paths: &AppPaths, name: &str, email: &str) -> Result<()> {
    validate_profile_name(name)?;
    validate_expected_email(email)?;
    let _profile_reservation = ProfileReservation::acquire(paths, name)?;
    let current_executable = env::current_exe().context("failed to locate this executable")?;
    let initial_real_claude = {
        let _state_lock = StateLock::acquire(paths)?;
        let state = state::load(paths)?;
        if state.profiles.contains_key(name) {
            bail!("profile `{name}` already exists");
        }
        if state
            .profiles
            .values()
            .any(|profile| matches!(profile.location, ProfileLocation::Default))
        {
            bail!("the default Claude Code account is already registered");
        }
        validate_new_profile_name(&state, name)?;
        state.real_claude.clone()
    };

    let real_claude =
        process::resolve_real_claude(initial_real_claude.as_deref(), &current_executable, paths)?;
    process::validate_platform_support(&real_claude)?;
    let profile = Profile::default(email);
    process::validate_profile_identity(&real_claude, &profile, paths)
        .with_context(|| format!("default account verification failed for profile `{name}`"))?;

    let first_profile;
    {
        let _state_lock = StateLock::acquire(paths)?;
        let mut state = state::load(paths)?;
        if state.profiles.contains_key(name) {
            bail!("profile `{name}` was added by another process");
        }
        if state
            .profiles
            .values()
            .any(|profile| matches!(profile.location, ProfileLocation::Default))
        {
            bail!("the default Claude Code account was registered by another process");
        }
        validate_new_profile_name(&state, name)?;
        first_profile = state.profiles.is_empty();
        if state.real_claude == initial_real_claude {
            state.real_claude = Some(real_claude);
        }
        state.profiles.insert(name.to_owned(), profile);
        if first_profile {
            state.active = Some(name.to_owned());
        }
        state::save(paths, &state)?;
    }

    if first_profile {
        println!("Adopted `{name}` and made it active.");
    } else {
        println!("Adopted `{name}`. Activate it with `claude account use {name}`.");
    }
    Ok(())
}

fn reauth(paths: &AppPaths, name: &str, sso: bool) -> Result<()> {
    validate_profile_name(name)?;
    let _profile_reservation = ProfileReservation::acquire(paths, name)?;
    let (profile, real_claude) = {
        let _state_lock = StateLock::acquire(paths)?;
        let state = state::load(paths)?;
        let profile = state
            .profiles
            .get(name)
            .cloned()
            .with_context(|| format!("profile `{name}` does not exist"))?;
        let real_claude = state
            .real_claude
            .clone()
            .context("real Claude executable is not configured")?;
        (profile, real_claude)
    };

    let current_executable = env::current_exe().context("failed to locate this executable")?;
    let real_claude = process::pin_real_claude_candidate(&real_claude, &current_executable, paths)?;
    process::validate_platform_support(&real_claude)?;
    println!("Logging in profile `{name}` using Claude Code...");
    let mut login = process::managed_command(&real_claude, &profile, paths)?;
    login.args(["auth", "login", "--email", &profile.email]);
    if sso {
        login.arg("--sso");
    }
    let status = login.status().context("failed to start Claude login")?;
    if !status.success() {
        bail!("Claude login failed for profile `{name}`");
    }
    process::validate_profile_identity(&real_claude, &profile, paths)
        .with_context(|| format!("login verification failed for profile `{name}`"))?;
    println!("Reauthenticated `{name}`.");
    Ok(())
}

fn complete_claude_onboarding(config_path: &Path) -> Result<()> {
    let mut config = match fs::read(config_path) {
        Ok(contents) => serde_json::from_slice::<Value>(&contents)
            .with_context(|| format!("failed to parse {}", config_path.display()))?,
        Err(error) if error.kind() == ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", config_path.display()));
        }
    };
    let config = config
        .as_object_mut()
        .with_context(|| format!("{} must contain a JSON object", config_path.display()))?;
    config.insert("hasCompletedOnboarding".to_owned(), Value::Bool(true));

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary =
        config_path.with_file_name(format!(".claude.json.tmp.{}.{}", std::process::id(), nonce));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, &config)
            .context("failed to serialize Claude onboarding state")?;
        file.write_all(b"\n")
            .context("failed to finish Claude onboarding state")?;
        file.sync_all()
            .context("failed to sync Claude onboarding state")?;
        fs::rename(&temporary, config_path)
            .with_context(|| format!("failed to update {}", config_path.display()))?;
        fs::set_permissions(config_path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect {}", config_path.display()))?;
        state::sync_parent_directory(config_path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn use_profile(paths: &AppPaths, name: &str) -> Result<()> {
    validate_profile_name(name)?;
    let _profile_reservation = ProfileReservation::acquire(paths, name)?;
    let _lock = StateLock::acquire(paths)?;
    let mut state = state::load(paths)?;
    if !state.profiles.contains_key(name) {
        bail!("profile `{name}` does not exist");
    }
    state.active = Some(name.to_owned());
    state::save(paths, &state)?;
    println!("Now using `{name}` for new Claude processes.");
    Ok(())
}

fn list(paths: &AppPaths) -> Result<()> {
    let state = state::load(paths)?;
    if state.profiles.is_empty() {
        println!("No profiles. Add one with `claude account add NAME --email EMAIL`.");
        return Ok(());
    }
    for name in state.profiles.keys() {
        let marker = if state.active.as_deref() == Some(name) {
            "*"
        } else {
            " "
        };
        println!("{marker} {name}");
    }
    Ok(())
}

fn current(paths: &AppPaths) -> Result<()> {
    let state = state::load(paths)?;
    match state.active {
        Some(name) => {
            println!("{name}");
            Ok(())
        }
        None => bail!("no active profile"),
    }
}

fn resolve_case_collision(paths: &AppPaths, name: &str) -> Result<()> {
    validate_profile_name(name)?;
    let _profile_reservation = ProfileReservation::acquire(paths, name)?;
    let _lock = StateLock::acquire(paths)?;
    let mut state = state::load_for_case_collision_resolution(paths)?;
    let profile = state
        .profiles
        .get(name)
        .cloned()
        .with_context(|| format!("profile `{name}` does not exist"))?;
    state
        .case_colliding_profile_name(name)
        .with_context(|| format!("profile `{name}` does not have a case-colliding sibling"))?;

    state.profiles.remove(name);
    let removed_was_active = state.active.as_deref() == Some(name);
    if removed_was_active {
        state.active = None;
    }
    let remaining_case_variants: Vec<String> = state
        .profiles
        .keys()
        .filter(|existing| existing.eq_ignore_ascii_case(name))
        .cloned()
        .collect();
    state::save(paths, &state)?;

    println!("Unregistered exact profile name `{name}` from account state.");
    println!(
        "Local data and credentials remain available. Claude logout did not run, and {} was not deleted.",
        profile.config_dir(paths).display()
    );
    if let [survivor] = remaining_case_variants.as_slice() {
        println!("`{survivor}` remains registered.");
        if removed_was_active {
            println!(
                "Finish recovery with `claude account use {survivor}` if it should be active."
            );
        } else if state.active.as_deref() == Some(survivor.as_str()) {
            println!("`{survivor}` remains active. Normal commands can resume.");
        } else {
            println!(
                "Normal commands can resume. Run `claude account use {survivor}` to activate the surviving profile."
            );
        }
    } else {
        let next = &remaining_case_variants[0];
        println!(
            "Case-colliding profiles still remain. Run `claude account resolve-case-collision {next}` again, leaving exactly one spelling registered."
        );
    }
    Ok(())
}

fn remove(paths: &AppPaths, name: &str, purge: bool, force: bool) -> Result<()> {
    remove_with_purge(paths, name, purge, force, |_| Ok(()))
}

fn remove_with_purge(
    paths: &AppPaths,
    name: &str,
    purge: bool,
    force: bool,
    purge_directory: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    validate_profile_name(name)?;
    if purge {
        bail!("`remove --purge` is disabled until profile deletion is transactional");
    }
    let _profile_reservation = ProfileReservation::acquire(paths, name)?;
    let (profile, real_claude) = {
        let _lock = StateLock::acquire(paths)?;
        let state = state::load(paths)?;
        let profile = state
            .profiles
            .get(name)
            .cloned()
            .with_context(|| format!("profile `{name}` does not exist"))?;
        let real_claude = state
            .real_claude
            .clone()
            .context("real Claude executable is not configured")?;
        let is_active = state.active.as_deref() == Some(name);
        if is_active && !force {
            bail!(
                "`{name}` is active. Select another profile, or use --force to leave no active profile."
            );
        }
        (profile, real_claude)
    };

    let current_executable = env::current_exe().context("failed to locate this executable")?;
    let real_claude = process::pin_real_claude_candidate(&real_claude, &current_executable, paths)?;
    process::validate_platform_support(&real_claude)?;
    println!("Logging out profile `{name}`...");
    let logout_status = process::managed_command(&real_claude, &profile, paths)?
        .args(["auth", "logout"])
        .status()
        .context("failed to start Claude logout")?;
    if !logout_status.success() {
        bail!("Claude logout failed. Profile `{name}` was not removed.");
    }

    {
        let _lock = StateLock::acquire(paths)?;
        let mut state = state::load(paths)?;
        state.profiles.remove(name);
        if state.active.as_deref() == Some(name) {
            state.active = None;
        }
        state::save(paths, &state)?;
    }

    let _ = purge_directory;
    println!(
        "Removed `{name}`. Its non-credential data remains at {}.",
        profile.config_dir(paths).display()
    );
    Ok(())
}

fn install(paths: &AppPaths, explicit_real: Option<&Path>) -> Result<()> {
    let current_executable = env::current_exe().context("failed to locate this executable")?;
    let _lock = StateLock::acquire(paths)?;
    let mut account_state = state::load(paths)?;
    let configured = account_state.real_claude.clone();
    let real_claude = match explicit_real {
        Some(path) => {
            if !path.is_absolute() {
                bail!("--real must be an absolute path");
            }
            process::pin_real_claude_candidate(path, &current_executable, paths)?
        }
        None => process::resolve_real_claude(configured.as_deref(), &current_executable, paths)?,
    };
    process::validate_platform_support(&real_claude)?;

    state::ensure_private_dir(&paths.data_dir)?;
    state::ensure_private_dir(&paths.shim_dir)?;
    let libexec_dir = paths
        .installed_executable
        .parent()
        .context("invalid installation path")?;
    state::ensure_private_dir(libexec_dir)?;

    let same_executable = fs::canonicalize(&current_executable).ok()
        == fs::canonicalize(&paths.installed_executable).ok();
    if !same_executable {
        if fs::symlink_metadata(&paths.installed_executable)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!(
                "refusing to replace managed symlink {}",
                paths.installed_executable.display()
            );
        }
        let temporary = paths
            .installed_executable
            .with_extension(format!("tmp.{}", std::process::id()));
        let mut destination = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o755)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        let mut source = fs::File::open(&current_executable)
            .with_context(|| format!("failed to open {}", current_executable.display()))?;
        std::io::copy(&mut source, &mut destination).context("failed to install executable")?;
        destination
            .sync_all()
            .context("failed to sync executable")?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
        fs::rename(&temporary, &paths.installed_executable)
            .context("failed to activate installed executable")?;
        state::sync_parent_directory(&paths.installed_executable)?;
    }
    fs::set_permissions(
        &paths.installed_executable,
        fs::Permissions::from_mode(0o755),
    )
    .context("failed to protect installed executable")?;

    let shim_is_current = if let Ok(metadata) = fs::symlink_metadata(&paths.shim) {
        let points_to_us = metadata.file_type().is_symlink()
            && fs::canonicalize(&paths.shim).ok()
                == fs::canonicalize(&paths.installed_executable).ok();
        if !points_to_us {
            bail!(
                "refusing to replace existing non-managed path {}",
                paths.shim.display()
            );
        }
        true
    } else {
        false
    };

    if !shim_is_current {
        let temporary_shim = paths
            .shim
            .with_extension(format!("tmp.{}", std::process::id()));
        if temporary_shim.exists() {
            bail!(
                "temporary installation path already exists: {}",
                temporary_shim.display()
            );
        }
        symlink(&paths.installed_executable, &temporary_shim)
            .context("failed to create Claude shim")?;
        fs::rename(&temporary_shim, &paths.shim).context("failed to activate Claude shim")?;
        state::sync_parent_directory(&paths.shim)?;
    }

    account_state.real_claude = Some(real_claude.clone());
    state::save(paths, &account_state)?;

    println!("Installed claude-account.");
    println!("Real Claude: {}", real_claude.display());
    println!("Shim: {}", paths.shim.display());
    println!();
    println!("Put this directory before the real Claude directory in PATH:");
    println!("{}", paths.shim_dir.display());
    Ok(())
}

fn validate_new_profile_name(state: &state::State, name: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    if let Some(existing) = state.case_colliding_profile_name(name) {
        bail!(
            "Profile `{name}` differs only by case from `{existing}`. Select another name on macOS."
        );
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (state, name);

    Ok(())
}

fn validate_profile_name(name: &str) -> Result<()> {
    if !state::is_valid_profile_name(name) {
        bail!(
            "invalid profile name `{name}`; use 1-32 letters, numbers, hyphens, or underscores, \
             starting with a letter or number"
        );
    }
    Ok(())
}

fn validate_expected_email(email: &str) -> Result<()> {
    if !state::is_valid_profile_email(email) {
        bail!("invalid subscription email address");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_name_validation_blocks_path_traversal() {
        for invalid in ["", "../work", ".work", "work space", "work/personal"] {
            assert!(validate_profile_name(invalid).is_err(), "{invalid}");
        }
        for valid in ["work", "personal-2", "team_account"] {
            assert!(validate_profile_name(valid).is_ok(), "{valid}");
        }
    }

    #[test]
    fn email_validation_rejects_unsafe_values() {
        for invalid in [
            "",
            "missing-at.example.com",
            "@example.com",
            "work@",
            "work@@example.com",
            "work name@example.com",
            "work\n@example.com",
        ] {
            assert!(!state::is_valid_profile_email(invalid), "{invalid:?}");
        }
        assert!(state::is_valid_profile_email("work@example.com"));
    }

    #[test]
    fn add_uses_an_isolated_config_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let fake_claude = temp.path().join("claude-real");
        let log = temp.path().join("calls.log");
        let mut script = fs::File::create(&fake_claude).unwrap();
        writeln!(
            script,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               printf '2.1.226 (Claude Code)\\n'\n\
               exit 0\n\
             fi\n\
             printf '%s|%s\\n' \"$CLAUDE_CONFIG_DIR\" \"$*\" >> '{}'\n\
             if [ \"$1 $2\" = \"auth login\" ]; then\n\
               printf '{{\"oauthAccount\":{{\"emailAddress\":\"work@example.com\"}}}}\\n' > \"$CLAUDE_CONFIG_DIR/.claude.json\"\n\
             fi\n\
             if [ \"$1 $2 $3\" = \"auth status --json\" ]; then\n\
               printf '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"work@example.com\",\"subscriptionType\":\"max\"}}\\n'\n\
             fi\n\
             exit 0",
            log.display()
        )
        .unwrap();
        drop(script);
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        {
            let _lock = StateLock::acquire(&paths).unwrap();
            let mut initial = state::load(&paths).unwrap();
            initial.real_claude = Some(fake_claude);
            state::save(&paths, &initial).unwrap();
        }

        add(&paths, "work", "work@example.com", false).unwrap();
        let calls = fs::read_to_string(log).unwrap();
        let expected = paths.profile_dir("work").display().to_string();
        assert!(calls.contains(&format!("{expected}|auth login")));
        assert!(calls.contains(&format!("{expected}|auth status --json")));
        let claude_config: Value = serde_json::from_slice(
            &fs::read(paths.profile_dir("work").join(".claude.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(claude_config["hasCompletedOnboarding"], true);
        assert_eq!(state::load(&paths).unwrap().active.as_deref(), Some("work"));
    }

    #[test]
    fn onboarding_update_preserves_existing_claude_state() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("profile");
        state::ensure_private_dir(&profile).unwrap();
        let config_path = profile.join(".claude.json");
        fs::write(
            &config_path,
            r#"{"existing":{"setting":"preserved"},"hasCompletedOnboarding":false}"#,
        )
        .unwrap();

        complete_claude_onboarding(&config_path).unwrap();

        let updated: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(updated["existing"]["setting"], "preserved");
        assert_eq!(updated["hasCompletedOnboarding"], true);
        assert_eq!(
            fs::metadata(config_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn add_rejects_successful_command_that_reports_logged_out() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let fake_claude = temp.path().join("claude-real");
        fs::write(
            &fake_claude,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               printf '2.1.226 (Claude Code)\\n'\n\
               exit 0\n\
             fi\n\
             if [ \"$1 $2 $3\" = \"auth status --json\" ]; then\n\
               printf '{\"loggedIn\":false}\\n'\n\
             fi\n\
             exit 0\n",
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        {
            let _lock = StateLock::acquire(&paths).unwrap();
            let mut initial = state::load(&paths).unwrap();
            initial.real_claude = Some(fake_claude);
            state::save(&paths, &initial).unwrap();
        }

        let error = add(&paths, "work", "work@example.com", false).unwrap_err();
        assert!(format!("{error:#}").contains("logged out"));
        assert!(!state::load(&paths).unwrap().profiles.contains_key("work"));
        assert!(!paths.profile_dir("work").join(".claude.json").exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn add_rejects_profile_name_that_differs_only_by_case() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let fake_claude = temp.path().join("claude-real");
        fs::write(
            &fake_claude,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then\n\
               printf '2.1.226 (Claude Code)\\n'\n\
               exit 0\n\
             fi\n\
             if [ \"$1 $2 $3\" = \"auth status --json\" ]; then\n\
               printf '{\"loggedIn\":true}\\n'\n\
             fi\n\
             exit 0\n",
        )
        .unwrap();
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

        {
            let _lock = StateLock::acquire(&paths).unwrap();
            let mut initial = state::load(&paths).unwrap();
            initial.real_claude = Some(fake_claude);
            initial
                .profiles
                .insert("Work".to_owned(), Profile::new(paths.profile_dir("Work")));
            initial.active = Some("Work".to_owned());
            state::save(&paths, &initial).unwrap();
        }

        let error = add(&paths, "work", "work@example.com", false).unwrap_err();
        assert!(error.to_string().contains("differs only by case"));
        assert_eq!(state::load(&paths).unwrap().profiles.len(), 1);
    }

    #[test]
    fn remove_purge_is_disabled_before_profile_changes() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let profile_dir = paths.profile_dir("work");
        state::ensure_private_dir(&profile_dir).unwrap();
        {
            let _lock = StateLock::acquire(&paths).unwrap();
            let mut initial = state::load(&paths).unwrap();
            initial
                .profiles
                .insert("work".to_owned(), Profile::new(profile_dir.clone()));
            state::save(&paths, &initial).unwrap();
        }

        let error = remove_with_purge(&paths, "work", true, false, |_| Ok(())).unwrap_err();

        assert!(error.to_string().contains("disabled"));
        assert!(state::load(&paths).unwrap().profiles.contains_key("work"));
        assert!(profile_dir.is_dir());
    }
}
