use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::AppPaths;

const LOCK_EX: i32 = 2;

unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default = "state_version")]
    pub version: u32,
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub real_claude: Option<PathBuf>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub email: String,
    pub location: ProfileLocation,
    pub created_at: u64,
    #[serde(
        rename = "authentication",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    imported_authentication: Option<ImportedAuthentication>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileLocation {
    Default,
    Isolated { config_dir: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ImportedAuthentication {
    #[serde(alias = "oauth")]
    OAuth,
    Api,
}

impl Profile {
    pub fn isolated(config_dir: PathBuf, email: impl Into<String>) -> Self {
        Self::with_location(ProfileLocation::Isolated { config_dir }, email)
    }

    pub fn default(email: impl Into<String>) -> Self {
        Self::with_location(ProfileLocation::Default, email)
    }

    fn with_location(location: ProfileLocation, email: impl Into<String>) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            email: email.into(),
            location,
            created_at,
            imported_authentication: None,
        }
    }

    #[cfg(test)]
    pub fn new(config_dir: PathBuf) -> Self {
        Self::isolated(config_dir, "test@example.com")
    }

    pub fn config_dir<'a>(&'a self, paths: &'a AppPaths) -> &'a Path {
        match &self.location {
            ProfileLocation::Default => &paths.default_claude_dir,
            ProfileLocation::Isolated { config_dir } => config_dir,
        }
    }

    pub fn claude_json_path(&self, paths: &AppPaths) -> PathBuf {
        match &self.location {
            ProfileLocation::Default => paths.default_claude_json.clone(),
            ProfileLocation::Isolated { config_dir } => config_dir.join(".claude.json"),
        }
    }
}

impl State {
    pub fn case_colliding_profile_name(&self, candidate: &str) -> Option<&str> {
        self.profiles.keys().find_map(|existing| {
            let existing = existing.as_str();
            (existing != candidate && existing.eq_ignore_ascii_case(candidate)).then_some(existing)
        })
    }
}

fn state_version() -> u32 {
    2
}

pub struct StateLock {
    _file: File,
}

pub struct ProfileReservation {
    _file: File,
}

#[derive(Clone, Copy)]
enum CaseCollisionValidation {
    Enforce,
    AllowForExplicitResolution,
}

impl StateLock {
    pub fn acquire(paths: &AppPaths) -> Result<Self> {
        ensure_private_dir(&paths.config_dir)?;
        reject_symlink(&paths.lock_file)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&paths.lock_file)
            .with_context(|| format!("failed to open {}", paths.lock_file.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect {}", paths.lock_file.display()))?;

        lock_exclusive(&file).context("failed to lock profile state")?;

        Ok(Self { _file: file })
    }
}

impl ProfileReservation {
    pub fn acquire(paths: &AppPaths, name: &str) -> Result<Self> {
        ensure_private_dir(&paths.profile_reservations_dir)?;
        let path = paths
            .profile_reservations_dir
            .join(format!("{}.lock", profile_reservation_key(name)));
        reject_symlink(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect {}", path.display()))?;
        lock_exclusive(&file)
            .with_context(|| format!("failed to reserve profile name `{name}`"))?;

        Ok(Self { _file: file })
    }
}

fn lock_exclusive(file: &File) -> io::Result<()> {
    loop {
        let result = unsafe { flock(file.as_raw_fd(), LOCK_EX) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn profile_reservation_key(name: &str) -> String {
    if cfg!(target_os = "macos") {
        name.to_ascii_lowercase()
    } else {
        name.to_owned()
    }
}

pub fn load(paths: &AppPaths) -> Result<State> {
    load_with_case_collision_validation(paths, CaseCollisionValidation::Enforce)
}

/// Load version-checked state while bypassing only the macOS case-collision check.
pub fn load_for_case_collision_resolution(paths: &AppPaths) -> Result<State> {
    load_with_case_collision_validation(paths, CaseCollisionValidation::AllowForExplicitResolution)
}

fn load_with_case_collision_validation(
    paths: &AppPaths,
    collision_validation: CaseCollisionValidation,
) -> Result<State> {
    let state = match reject_symlink(&paths.state_file).and_then(|()| {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&paths.state_file)
            .with_context(|| format!("failed to read {}", paths.state_file.display()))
    }) {
        Ok(file) => {
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to protect {}", paths.state_file.display()))?;
            let value: serde_json::Value = serde_json::from_reader(file)
                .with_context(|| format!("failed to parse {}", paths.state_file.display()))?;
            let version = value
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_else(|| u64::from(state_version()));
            match version {
                1 => migrate_v1(value)?,
                2 => serde_json::from_value(value)
                    .with_context(|| format!("failed to parse {}", paths.state_file.display()))?,
                other => anyhow::bail!("unsupported state version {other}"),
            }
        }
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            State {
                version: state_version(),
                ..State::default()
            }
        }
        Err(error) => return Err(error),
    };
    validate_platform_state(paths, &state, collision_validation)?;
    Ok(state)
}

#[derive(Deserialize)]
struct LegacyState {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    real_claude: Option<PathBuf>,
    #[serde(default)]
    profiles: BTreeMap<String, LegacyProfile>,
}

#[derive(Deserialize)]
struct LegacyProfile {
    config_dir: PathBuf,
    created_at: u64,
    #[serde(rename = "authentication", default)]
    imported_authentication: Option<ImportedAuthentication>,
}

fn migrate_v1(value: serde_json::Value) -> Result<State> {
    let legacy: LegacyState =
        serde_json::from_value(value).context("failed to parse version 1 state")?;
    let mut profiles = BTreeMap::new();
    for (name, profile) in legacy.profiles {
        if profile.imported_authentication == Some(ImportedAuthentication::Api) {
            anyhow::bail!(
                "Profile `{name}` is an API profile. This release supports subscriptions and will not read or migrate its API key."
            );
        }
        let claude_json = profile.config_dir.join(".claude.json");
        let email = read_profile_email(&claude_json).with_context(|| {
            format!(
                "cannot migrate profile `{name}` without its account email in {}",
                claude_json.display()
            )
        })?;
        profiles.insert(
            name,
            Profile {
                email,
                location: ProfileLocation::Isolated {
                    config_dir: profile.config_dir,
                },
                created_at: profile.created_at,
                imported_authentication: profile.imported_authentication,
            },
        );
    }
    Ok(State {
        version: state_version(),
        active: legacy.active,
        real_claude: legacy.real_claude,
        profiles,
    })
}

fn read_profile_email(path: &Path) -> Result<String> {
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("failed to read {}", path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", path.display()))?;
    let email = value
        .get("oauthAccount")
        .and_then(|account| account.get("emailAddress"))
        .and_then(serde_json::Value::as_str)
        .filter(|email| !email.is_empty())
        .context("oauthAccount.emailAddress is missing")?;
    Ok(email.to_owned())
}

fn validate_platform_state(
    paths: &AppPaths,
    state: &State,
    collision_validation: CaseCollisionValidation,
) -> Result<()> {
    if let Some((name, _)) = state
        .profiles
        .iter()
        .find(|(_, profile)| profile.imported_authentication == Some(ImportedAuthentication::Api))
    {
        anyhow::bail!(
            "Profile `{name}` is an API profile from claude-account-macos. This release will not read or migrate its API key. Use claude-account-macos for that profile."
        );
    }

    if let Some(active) = state.active.as_deref() {
        if !state.profiles.contains_key(active) {
            anyhow::bail!("active profile `{active}` does not exist");
        }
    }

    if state
        .real_claude
        .as_deref()
        .is_some_and(|path| !path.is_absolute())
    {
        anyhow::bail!("the stored real Claude path must be absolute");
    }

    let mut default_profile = None;
    for (name, profile) in &state.profiles {
        if !is_valid_profile_name(name) {
            anyhow::bail!("state contains invalid profile name `{name}`");
        }
        if !is_valid_profile_email(&profile.email) {
            anyhow::bail!("profile `{name}` contains an invalid email address");
        }
        match &profile.location {
            ProfileLocation::Default => {
                if let Some(existing) = default_profile {
                    anyhow::bail!(
                        "profiles `{existing}` and `{name}` both use the default Claude location"
                    );
                }
                default_profile = Some(name.as_str());
            }
            ProfileLocation::Isolated { config_dir } => {
                let expected = paths.profile_dir(name);
                if config_dir != &expected {
                    anyhow::bail!(
                        "profile `{name}` must use managed directory {}",
                        expected.display()
                    );
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if matches!(collision_validation, CaseCollisionValidation::Enforce) {
            for name in state.profiles.keys() {
                if let Some(existing) = state.case_colliding_profile_name(name) {
                    anyhow::bail!(
                        "Profiles `{name}` and `{existing}` differ only by case. macOS profile names must be case-insensitively unique. Run `claude account resolve-case-collision {name}`."
                    );
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (state, collision_validation);

    Ok(())
}

pub fn is_valid_profile_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
        && name.len() <= 32
}

pub fn is_valid_profile_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && email.len() <= 254
        && !email
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

pub fn save(paths: &AppPaths, state: &State) -> Result<()> {
    ensure_private_dir(&paths.config_dir)?;
    let temporary = temporary_state_path(&paths.state_file);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, state).context("failed to serialize state")?;
        file.write_all(b"\n")
            .context("failed to finish state file")?;
        file.sync_all().context("failed to sync state file")?;
        fs::rename(&temporary, &paths.state_file).with_context(|| {
            format!(
                "failed to replace {} with {}",
                paths.state_file.display(),
                temporary.display()
            )
        })?;
        fs::set_permissions(&paths.state_file, fs::Permissions::from_mode(0o600))
            .context("failed to protect state file")?;
        sync_parent_directory(&paths.state_file)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("managed directory must be absolute: {}", path.display());
    }

    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => {
                anyhow::bail!("managed directory cannot contain `..`: {}", path.display())
            }
            Component::Normal(name) => {
                current.push(name);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        if current == path || !allowed_system_symlink(&current) {
                            anyhow::bail!(
                                "managed directory has a symlink component: {}",
                                current.display()
                            );
                        }
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        anyhow::bail!(
                            "managed directory component is not a directory: {}",
                            current.display()
                        )
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        match fs::create_dir(&current) {
                            Ok(()) => {}
                            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                                let metadata =
                                    fs::symlink_metadata(&current).with_context(|| {
                                        format!("failed to recheck directory {}", current.display())
                                    })?;
                                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                                    anyhow::bail!(
                                        "concurrent creation produced an unsafe path: {}",
                                        current.display()
                                    );
                                }
                            }
                            Err(error) => {
                                return Err(error).with_context(|| {
                                    format!("failed to create directory {}", current.display())
                                });
                            }
                        }
                        fs::set_permissions(&current, fs::Permissions::from_mode(0o700))
                            .with_context(|| {
                                format!("failed to protect directory {}", current.display())
                            })?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to inspect directory {}", current.display())
                        });
                    }
                }
            }
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to protect directory {}", path.display()))?;
    Ok(())
}

fn allowed_system_symlink(path: &Path) -> bool {
    cfg!(target_os = "macos") && matches!(path.to_str(), Some("/tmp" | "/var" | "/etc"))
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("refusing managed symlink: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

pub fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("managed path does not have a parent")?;
    File::open(parent)
        .with_context(|| format!("failed to open directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("failed to sync directory {}", parent.display()))
}

fn temporary_state_path(state_file: &Path) -> PathBuf {
    let filename = state_file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    state_file.with_file_name(format!("{filename}.tmp.{}.{}", std::process::id(), nonce))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_reservation_keys_serialize_names_according_to_platform() {
        let upper = profile_reservation_key("Work");
        let lower = profile_reservation_key("work");
        if cfg!(target_os = "macos") {
            assert_eq!(upper, "work");
            assert_eq!(upper, lower);
        } else {
            assert_eq!(upper, "Work");
            assert_ne!(upper, lower);
        }
    }

    #[test]
    fn state_round_trip_preserves_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let mut state = State {
            version: 2,
            ..State::default()
        };
        state.active = Some("work".to_owned());
        state
            .profiles
            .insert("work".to_owned(), Profile::new(paths.profile_dir("work")));

        let _lock = StateLock::acquire(&paths).unwrap();
        save(&paths, &state).unwrap();
        let loaded = load(&paths).unwrap();

        assert_eq!(loaded.active.as_deref(), Some("work"));
        assert!(loaded.profiles.contains_key("work"));
    }

    #[test]
    fn imported_oauth_profile_remains_compatible() {
        for authentication in ["o_auth", "oauth"] {
            let profile: LegacyProfile = serde_json::from_value(serde_json::json!({
                "config_dir": "/tmp/work",
                "created_at": 0,
                "authentication": authentication
            }))
            .unwrap();

            assert_eq!(
                profile.imported_authentication,
                Some(ImportedAuthentication::OAuth)
            );
        }
    }

    #[test]
    fn load_migrates_version_one_profiles_with_account_email() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let profile_dir = paths.profile_dir("work");
        ensure_private_dir(&profile_dir).unwrap();
        fs::write(
            profile_dir.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"work@example.com"}}"#,
        )
        .unwrap();
        let _lock = StateLock::acquire(&paths).unwrap();
        fs::write(
            &paths.state_file,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "active": "work",
                "profiles": {
                    "work": {"config_dir": profile_dir, "created_at": 1}
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let migrated = load(&paths).unwrap();

        assert_eq!(migrated.version, 2);
        let profile = migrated.profiles.get("work").unwrap();
        assert_eq!(profile.email, "work@example.com");
        assert!(matches!(profile.location, ProfileLocation::Isolated { .. }));
    }

    #[test]
    fn locks_and_state_files_reset_private_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let lock = StateLock::acquire(&paths).unwrap();
        let state = State {
            version: 2,
            ..State::default()
        };
        save(&paths, &state).unwrap();
        fs::set_permissions(&paths.lock_file, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&paths.state_file, fs::Permissions::from_mode(0o644)).unwrap();
        drop(lock);

        let _lock = StateLock::acquire(&paths).unwrap();
        load(&paths).unwrap();

        assert_eq!(
            fs::metadata(&paths.lock_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&paths.state_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn private_directory_rejects_a_symlink_component() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();

        for path in [link.clone(), link.join("child")] {
            let error = ensure_private_dir(&path).unwrap_err();
            assert!(error.to_string().contains("symlink component"));
        }
        assert!(!target.join("child").exists());
    }

    #[test]
    fn profile_reservation_rejects_a_symlink_lock_file() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        ensure_private_dir(&paths.profile_reservations_dir).unwrap();
        let target = temp.path().join("target");
        fs::write(&target, b"preserve\n").unwrap();
        symlink(&target, paths.profile_reservations_dir.join("work.lock")).unwrap();

        let error = ProfileReservation::acquire(&paths, "work").err().unwrap();

        assert!(error.to_string().contains("managed symlink"));
        assert_eq!(fs::read(target).unwrap(), b"preserve\n");
    }

    #[test]
    fn load_rejects_imported_api_profiles_without_reading_their_keys() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let _lock = StateLock::acquire(&paths).unwrap();
        fs::write(
            &paths.state_file,
            format!(
                r#"{{"version":1,"profiles":{{"gateway":{{"config_dir":"{}","created_at":0,"authentication":"api"}}}}}}"#,
                paths.profile_dir("gateway").display()
            ),
        )
        .unwrap();

        let error = load(&paths).unwrap_err();

        assert!(error.to_string().contains("API profile"));
        assert!(error
            .to_string()
            .contains("will not read or migrate its API key"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn load_rejects_case_colliding_profile_names() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let mut state = State {
            version: 2,
            ..State::default()
        };
        state
            .profiles
            .insert("work".to_owned(), Profile::new(paths.profile_dir("work")));
        state
            .profiles
            .insert("Work".to_owned(), Profile::new(paths.profile_dir("Work")));

        save(&paths, &state).unwrap();
        let error = load(&paths).unwrap_err();

        assert!(error.to_string().contains("differ only by case"));
    }

    #[test]
    fn collision_resolution_load_still_rejects_unsupported_versions() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let _lock = StateLock::acquire(&paths).unwrap();
        fs::write(
            &paths.state_file,
            r#"{"version":3,"active":null,"real_claude":null,"profiles":{}}"#,
        )
        .unwrap();

        let error = load_for_case_collision_resolution(&paths).unwrap_err();

        assert!(error.to_string().contains("unsupported state version 3"));
    }

    #[test]
    fn collision_resolution_load_still_rejects_malformed_state() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let _lock = StateLock::acquire(&paths).unwrap();
        fs::write(&paths.state_file, b"not json\n").unwrap();

        let error = load_for_case_collision_resolution(&paths).unwrap_err();

        assert!(error.to_string().contains("failed to parse"));
    }

    #[test]
    fn load_rejects_an_isolated_path_outside_the_managed_profile_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let _lock = StateLock::acquire(&paths).unwrap();
        let mut state = State {
            version: 2,
            ..State::default()
        };
        state.profiles.insert(
            "work".to_owned(),
            Profile::isolated(temp.path().join("unrelated"), "work@example.com"),
        );
        save(&paths, &state).unwrap();

        let error = load(&paths).unwrap_err();

        assert!(error.to_string().contains("must use managed directory"));
    }

    #[test]
    fn load_rejects_invalid_profile_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let _lock = StateLock::acquire(&paths).unwrap();
        let mut state = State {
            version: 2,
            ..State::default()
        };
        state.profiles.insert(
            "../work".to_owned(),
            Profile::isolated(paths.profile_dir("../work"), "work@example.com"),
        );
        save(&paths, &state).unwrap();
        let invalid_name = load(&paths).unwrap_err();
        assert!(invalid_name
            .to_string()
            .contains("invalid profile name `../work`"));

        let mut state = State {
            version: 2,
            ..State::default()
        };
        state.profiles.insert(
            "work".to_owned(),
            Profile::isolated(paths.profile_dir("work"), "invalid email"),
        );
        save(&paths, &state).unwrap();
        let invalid_email = load(&paths).unwrap_err();
        assert!(invalid_email.to_string().contains("invalid email address"));

        let state = State {
            version: 2,
            active: Some("missing".to_owned()),
            ..State::default()
        };
        save(&paths, &state).unwrap();
        let missing_active = load(&paths).unwrap_err();
        assert!(missing_active
            .to_string()
            .contains("active profile `missing` does not exist"));

        let state = State {
            version: 2,
            real_claude: Some(PathBuf::from("relative/claude")),
            ..State::default()
        };
        save(&paths, &state).unwrap();
        let relative_executable = load(&paths).unwrap_err();
        assert!(relative_executable
            .to_string()
            .contains("real Claude path must be absolute"));
    }

    #[test]
    fn load_rejects_multiple_default_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_roots(temp.path().join("config"), temp.path().join("data"));
        let _lock = StateLock::acquire(&paths).unwrap();
        let mut state = State {
            version: 2,
            ..State::default()
        };
        state
            .profiles
            .insert("main".to_owned(), Profile::default("main@example.com"));
        state
            .profiles
            .insert("second".to_owned(), Profile::default("second@example.com"));
        save(&paths, &state).unwrap();

        let error = load(&paths).unwrap_err();

        assert!(error.to_string().contains("both use the default"));
    }
}
