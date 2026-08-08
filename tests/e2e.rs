use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn invoke(
    program: &Path,
    account_home: &Path,
    home: &Path,
    arguments: &[&OsStr],
    environment: &[(&str, &OsStr)],
) -> Output {
    let mut command = Command::new(program);
    command
        .env("CLAUDE_ACCOUNT_HOME", account_home)
        .env("HOME", home)
        .args(arguments);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn run(program: &Path, account_home: &Path, home: &Path, arguments: &[&str]) -> Output {
    let arguments: Vec<&OsStr> = arguments.iter().map(OsStr::new).collect();
    let output = invoke(program, account_home, home, &arguments, &[]);
    assert!(
        output.status.success(),
        "command failed: {}\nstdout: {}\nstderr: {}",
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn write_fake_claude(path: &Path, calls: &Path) {
    fs::write(
        path,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '2.1.226 (Claude Code)\n'
  exit 0
fi
if [ -n "$CLAUDE_CONFIG_DIR" ]; then
  config_dir="$CLAUDE_CONFIG_DIR"
  account_file="$CLAUDE_CONFIG_DIR/.claude.json"
  location="isolated"
else
  config_dir="$HOME/.claude"
  account_file="$HOME/.claude.json"
  location="default"
fi
printf '%s|%s|%s|%s|%s\n' "$location" "$CLAUDE_CONFIG_DIR" "$CLAUDE_SECURESTORAGE_CONFIG_DIR" "$ANTHROPIC_CONFIG_DIR" "$*" >> '{}'
if [ "$1 $2" = "auth login" ]; then
  email=""
  previous=""
  for argument in "$@"; do
    if [ "$previous" = "--email" ]; then
      email="$argument"
    fi
    previous="$argument"
  done
  [ -n "$email" ] || exit 64
  mkdir -p "$config_dir"
  printf '{{"oauthAccount":{{"emailAddress":"%s"}}}}\n' "$email" > "$account_file"
  exit 0
fi
if [ "$1 $2 $3" = "auth status --json" ]; then
  [ -f "$account_file" ] || exit 1
  email="$(sed -n 's/.*"emailAddress"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$account_file")"
  [ -n "$email" ] || exit 1
  printf '{{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"%s","subscriptionType":"max"}}\n' "$email"
  exit 0
fi
if [ "$1 $2" = "auth logout" ]; then
  exit 0
fi
if [ -n "$FORWARD_LOG" ]; then
  printf '%s\n' "$@" > "$FORWARD_LOG"
fi
printf 'forwarded:%s\n' "$*"
"#,
            calls.display()
        ),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn install(binary: &Path, fake_claude: &Path, account_home: &Path, home: &Path) -> PathBuf {
    run(
        binary,
        account_home,
        home,
        &["install", "--real", fake_claude.to_str().unwrap()],
    );
    account_home.join("bin/claude")
}

#[test]
fn isolated_profile_lifecycle_and_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let shim = install(binary, &fake_claude, &account_home, &home);

    run(
        &shim,
        &account_home,
        &home,
        &["account", "add", "work", "--email", "work@example.com"],
    );
    run(
        &shim,
        &account_home,
        &home,
        &[
            "account",
            "add",
            "personal",
            "--email",
            "personal@example.com",
        ],
    );
    run(&shim, &account_home, &home, &["account", "use", "personal"]);

    let forwarded = run(
        &shim,
        &account_home,
        &home,
        &["fix", "this", "--model", "sonnet"],
    );
    assert!(
        String::from_utf8_lossy(&forwarded.stdout).contains("forwarded:fix this --model sonnet")
    );

    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(account_home.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["version"], 2);
    assert_eq!(state["active"], "personal");
    assert_eq!(state["profiles"]["work"]["email"], "work@example.com");
    assert_eq!(
        state["profiles"]["personal"]["location"]["type"],
        "isolated"
    );

    run(&shim, &account_home, &home, &["account", "remove", "work"]);
    assert!(account_home.join("profiles/work").is_dir());

    let purge = invoke(
        &shim,
        &account_home,
        &home,
        &[
            OsStr::new("account"),
            OsStr::new("remove"),
            OsStr::new("personal"),
            OsStr::new("--force"),
            OsStr::new("--purge"),
            OsStr::new("--yes"),
        ],
        &[],
    );
    assert!(!purge.status.success());
    assert!(String::from_utf8_lossy(&purge.stderr).contains("disabled"));
}

#[test]
fn default_and_isolated_profiles_use_their_exact_locations() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&home).unwrap();
    fs::create_dir(home.join(".claude")).unwrap();
    fs::write(
        home.join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"main@example.com"}}"#,
    )
    .unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let shim = install(binary, &fake_claude, &account_home, &home);

    run(
        &shim,
        &account_home,
        &home,
        &[
            "account",
            "adopt-default",
            "main",
            "--email",
            "main@example.com",
        ],
    );
    run(
        &shim,
        &account_home,
        &home,
        &["account", "add", "second", "--email", "second@example.com"],
    );
    run(&shim, &account_home, &home, &["account", "use", "main"]);
    run(&shim, &account_home, &home, &["hello-main"]);
    run(&shim, &account_home, &home, &["account", "use", "second"]);
    run(&shim, &account_home, &home, &["hello-second"]);

    let calls = fs::read_to_string(calls).unwrap();
    let default_anthropic = home.join(".claude/.anthropic");
    let isolated = account_home.join("profiles/second");
    assert!(calls.contains(&format!(
        "default|||{}|auth status --json",
        default_anthropic.display()
    )));
    assert!(calls.contains(&format!(
        "isolated|{}|{}|{}/.anthropic|auth status --json",
        isolated.display(),
        isolated.display(),
        isolated.display()
    )));
    assert!(calls.contains("default|||"));
    assert!(calls.contains("hello-main"));
    assert!(calls.contains("hello-second"));
}

#[test]
fn identity_failure_blocks_normal_launch_but_recovery_remains_available() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let shim = install(binary, &fake_claude, &account_home, &home);
    run(
        &shim,
        &account_home,
        &home,
        &["account", "add", "work", "--email", "work@example.com"],
    );
    fs::write(
        account_home.join("profiles/work/.claude.json"),
        r#"{"oauthAccount":{"emailAddress":"other@example.com"}}"#,
    )
    .unwrap();

    let blocked = invoke(
        &shim,
        &account_home,
        &home,
        &[OsStr::new("normal-command")],
        &[],
    );
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("work@example.com"));
    assert!(!fs::read_to_string(&calls)
        .unwrap()
        .lines()
        .any(|line| line.ends_with("|normal-command")));

    run(&shim, &account_home, &home, &["auth", "status", "--json"]);
    run(&shim, &account_home, &home, &["account", "reauth", "work"]);
    run(&shim, &account_home, &home, &["normal-command"]);
}

#[test]
fn install_is_serialized_idempotent_and_rejects_recursion() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));

    let mut children = Vec::new();
    for _ in 0..2 {
        children.push(
            Command::new(binary)
                .env("CLAUDE_ACCOUNT_HOME", &account_home)
                .env("HOME", &home)
                .args(["install", "--real"])
                .arg(&fake_claude)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    run(
        binary,
        &account_home,
        &home,
        &["install", "--real", fake_claude.to_str().unwrap()],
    );
    let shim = account_home.join("bin/claude");
    assert_eq!(
        fs::canonicalize(&shim).unwrap(),
        fs::canonicalize(account_home.join("libexec/claude-account")).unwrap()
    );

    let recursive = invoke(
        binary,
        &account_home,
        &home,
        &[
            OsStr::new("install"),
            OsStr::new("--real"),
            shim.as_os_str(),
        ],
        &[],
    );
    assert!(!recursive.status.success());
    assert!(String::from_utf8_lossy(&recursive.stderr).contains("managed claude-account path"));
}

#[test]
fn concurrent_use_waits_for_remove_and_cannot_restore_the_profile() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let remove_started = temp.path().join("remove-started");
    let release_remove = temp.path().join("release-remove");
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    fs::create_dir(&home).unwrap();
    fs::write(
        &fake_claude,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '2.1.226 (Claude Code)\\n'; exit 0; fi\nif [ \"$1 $2\" = \"auth logout\" ]; then : > '{}'; count=0; while [ ! -e '{}' ] && [ \"$count\" -lt 500 ]; do count=$((count + 1)); sleep 0.01; done; [ -e '{}' ] || exit 70; fi\n",
            remove_started.display(),
            release_remove.display(),
            release_remove.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
    let shim = account_home.join("bin/claude");
    let work = account_home.join("profiles/work");
    let personal = account_home.join("profiles/personal");
    fs::create_dir_all(shim.parent().unwrap()).unwrap();
    fs::create_dir_all(&work).unwrap();
    fs::create_dir_all(&personal).unwrap();
    symlink(binary, &shim).unwrap();
    fs::write(
        account_home.join("state.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "active": "personal",
            "real_claude": fake_claude,
            "profiles": {
                "personal": {
                    "email": "personal@example.com",
                    "location": {"type": "isolated", "config_dir": personal},
                    "created_at": 0
                },
                "work": {
                    "email": "work@example.com",
                    "location": {"type": "isolated", "config_dir": work},
                    "created_at": 0
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let removing = Command::new(&shim)
        .env("CLAUDE_ACCOUNT_HOME", &account_home)
        .env("HOME", &home)
        .args(["account", "remove", "work"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(wait_until(Duration::from_secs(10), || remove_started.exists()));
    let mut selecting = Command::new(&shim)
        .env("CLAUDE_ACCOUNT_HOME", &account_home)
        .env("HOME", &home)
        .args(["account", "use", "work"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(selecting.try_wait().unwrap().is_none());

    fs::write(&release_remove, b"release\n").unwrap();
    let remove_output = removing.wait_with_output().unwrap();
    let use_output = selecting.wait_with_output().unwrap();
    assert!(
        remove_output.status.success(),
        "{}",
        String::from_utf8_lossy(&remove_output.stderr)
    );
    assert!(!use_output.status.success());
    assert!(String::from_utf8_lossy(&use_output.stderr).contains("does not exist"));
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(account_home.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["active"], "personal");
    assert!(state["profiles"].get("work").is_none());
}

#[test]
fn add_merges_a_profile_selection_that_occurs_during_login() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let login_started = temp.path().join("login-started");
    let release_login = temp.path().join("release-login");
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    fs::create_dir(&home).unwrap();
    fs::write(
        &fake_claude,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '2.1.226 (Claude Code)\\n'; exit 0; fi\nif [ \"$1 $2\" = \"auth login\" ]; then : > '{}'; count=0; while [ ! -e '{}' ] && [ \"$count\" -lt 500 ]; do count=$((count + 1)); sleep 0.01; done; [ -e '{}' ] || exit 70; printf '{{\"oauthAccount\":{{\"emailAddress\":\"new@example.com\"}}}}\\n' > \"$CLAUDE_CONFIG_DIR/.claude.json\"; exit 0; fi\nif [ \"$1 $2 $3\" = \"auth status --json\" ]; then printf '{{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"apiProvider\":\"firstParty\",\"email\":\"new@example.com\",\"subscriptionType\":\"max\"}}\\n'; fi\n",
            login_started.display(),
            release_login.display(),
            release_login.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();
    install(binary, &fake_claude, &account_home, &home);
    let existing = account_home.join("profiles/existing");
    fs::create_dir_all(&existing).unwrap();
    let state_file = account_home.join("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_file).unwrap()).unwrap();
    state["active"] = serde_json::Value::Null;
    state["profiles"] = serde_json::json!({
        "existing": {
            "email": "existing@example.com",
            "location": {"type": "isolated", "config_dir": existing},
            "created_at": 0
        }
    });
    fs::write(&state_file, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

    let adding = Command::new(binary)
        .env("CLAUDE_ACCOUNT_HOME", &account_home)
        .env("HOME", &home)
        .args(["add", "new", "--email", "new@example.com"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(wait_until(Duration::from_secs(10), || login_started.exists()));
    run(binary, &account_home, &home, &["use", "existing"]);
    fs::write(&release_login, b"release\n").unwrap();
    let add_output = adding.wait_with_output().unwrap();
    assert!(
        add_output.status.success(),
        "{}",
        String::from_utf8_lossy(&add_output.stderr)
    );
    let final_state: serde_json::Value =
        serde_json::from_slice(&fs::read(state_file).unwrap()).unwrap();
    assert_eq!(final_state["active"], "existing");
    assert!(final_state["profiles"].get("existing").is_some());
    assert!(final_state["profiles"].get("new").is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn legacy_case_collision_fails_until_explicit_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let shim = account_home.join("bin/claude");
    let work_upper = account_home.join("profiles/Work");
    let work_lower = account_home.join("profiles/work");
    fs::create_dir(&home).unwrap();
    fs::create_dir_all(shim.parent().unwrap()).unwrap();
    fs::create_dir_all(&work_upper).unwrap();
    fs::write(work_upper.join("preserve"), b"keep\n").unwrap();
    symlink(binary, &shim).unwrap();
    fs::write(
        account_home.join("state.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "active": "work",
            "profiles": {
                "Work": {
                    "email": "work@example.com",
                    "location": {"type": "isolated", "config_dir": work_upper},
                    "created_at": 0
                },
                "work": {
                    "email": "work@example.com",
                    "location": {"type": "isolated", "config_dir": work_lower},
                    "created_at": 0
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let blocked = invoke(
        &shim,
        &account_home,
        &home,
        &[OsStr::new("account"), OsStr::new("list")],
        &[],
    );
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("resolve-case-collision"));
    run(
        &shim,
        &account_home,
        &home,
        &["account", "resolve-case-collision", "work"],
    );
    assert_eq!(fs::read(work_upper.join("preserve")).unwrap(), b"keep\n");
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(account_home.join("state.json")).unwrap()).unwrap();
    assert!(state["profiles"].get("work").is_none());
    assert!(state["profiles"].get("Work").is_some());
}

#[cfg(target_os = "macos")]
#[test]
fn version_probe_timeout_terminates_its_descendant() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let descendant_pid = temp.path().join("descendant.pid");
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    fs::create_dir(&home).unwrap();
    fs::write(
        &fake_claude,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '2.1.226 (Claude Code)\\n'; /bin/sleep 60 2>/dev/null & printf '%s\\n' \"$!\" > '{}'; exit 0; fi\n",
            descendant_pid.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let output = invoke(
        binary,
        &account_home,
        &home,
        &[
            OsStr::new("install"),
            OsStr::new("--real"),
            fake_claude.as_os_str(),
        ],
        &[],
    );
    assert!(!output.status.success());
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    let pid = fs::read_to_string(descendant_pid).unwrap();
    let pid = pid.trim();
    let mut running = Command::new("/bin/kill")
        .args(["-0", pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if running {
        std::thread::sleep(Duration::from_millis(200));
        running = Command::new("/bin/kill")
            .args(["-0", pid])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    }
    if running {
        let _ = Command::new("/bin/kill").args(["-9", pid]).status();
    }
    assert!(!running, "version probe descendant remained alive");
}

#[test]
fn forwards_non_utf8_arguments_without_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    let forwarded = temp.path().join("forwarded.bin");
    fs::create_dir(&home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let shim = install(binary, &fake_claude, &account_home, &home);
    run(
        &shim,
        &account_home,
        &home,
        &["account", "add", "work", "--email", "work@example.com"],
    );

    let invalid = OsString::from_vec(b"bad\x80".to_vec());
    let output = invoke(
        &shim,
        &account_home,
        &home,
        &[invalid.as_os_str(), OsStr::new("--flag")],
        &[("FORWARD_LOG", forwarded.as_os_str())],
    );

    assert!(output.status.success());
    assert_eq!(fs::read(forwarded).unwrap(), b"bad\x80\n--flag\n");
}

#[test]
fn managed_roots_reject_symlink_targets() {
    let temp = tempfile::tempdir().unwrap();
    let real_home = temp.path().join("real-account");
    let linked_home = temp.path().join("linked-account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&real_home).unwrap();
    fs::create_dir(&home).unwrap();
    symlink(&real_home, &linked_home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));

    let output = invoke(
        binary,
        &linked_home,
        &home,
        &[
            OsStr::new("install"),
            OsStr::new("--real"),
            fake_claude.as_os_str(),
        ],
        &[],
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("symlink component"));
}

#[test]
fn normal_launch_rejects_a_stored_executable_retargeted_to_the_shim() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let shim = install(binary, &fake_claude, &account_home, &home);
    run(
        &shim,
        &account_home,
        &home,
        &["account", "add", "work", "--email", "work@example.com"],
    );

    fs::remove_file(&fake_claude).unwrap();
    symlink(&shim, &fake_claude).unwrap();
    let output = invoke(
        &shim,
        &account_home,
        &home,
        &[OsStr::new("normal-command")],
        &[],
    );

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("wrapper"));
}

#[test]
fn help_requires_email_and_omits_console_login() {
    let temp = tempfile::tempdir().unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let output = run(binary, temp.path(), temp.path(), &["add", "--help"]);
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--email <EMAIL>"));
    assert!(!help.contains("--console"));
}

#[test]
fn bounded_install_completes_without_deadlock() {
    let temp = tempfile::tempdir().unwrap();
    let account_home = temp.path().join("account");
    let home = temp.path().join("home");
    let fake_claude = temp.path().join("real-claude");
    let calls = temp.path().join("calls.log");
    fs::create_dir(&home).unwrap();
    write_fake_claude(&fake_claude, &calls);
    let binary = Path::new(env!("CARGO_BIN_EXE_claude-account"));
    let started = Instant::now();
    install(binary, &fake_claude, &account_home, &home);
    assert!(started.elapsed() < Duration::from_secs(5));
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    predicate()
}
