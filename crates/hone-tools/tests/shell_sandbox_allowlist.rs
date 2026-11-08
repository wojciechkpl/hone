#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Contract tests (v2) for the shell tool's sandbox read/write allowlist,
//! enforcement opt-in, and command construction.
//!
//! Assumptions:
//! - `parse_extra_read("/c/")` returns `PathBuf::from("/c/")` unchanged.
//! - The test environment does NOT set `HONE_SANDBOX_ENFORCE` (we never mutate
//!   env in tests because it is racy).
//! - Extra roots in "accepted" tests live under `CARGO_TARGET_TMPDIR`, not the
//!   system temp dir, because extras under `/tmp` are rejected by contract and
//!   `tempfile::tempdir()` is under `/tmp` on Linux.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hone_sandbox::{
    Sandbox, SandboxError, SandboxHandle, SandboxMode, SandboxPlatform, SandboxPolicy,
};
use hone_tools::{
    EXTRA_READ_ENV, HOME_READ_DIRS, SANDBOX_ENFORCE_ENV, SYSTEM_READ_ROOTS, WRITE_DEVICES,
    parse_extra_read, sandbox_enforced, shell_sandbox_policy,
};

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A temp dir guaranteed not to be under the system temp dir (`/tmp`).
fn ext() -> tempfile::TempDir {
    tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("tempdir in CARGO_TARGET_TMPDIR")
}

/// A temp dir literally under `/tmp` (macOS `TempDir::new` uses /var/folders).
fn tmp_under_slash_tmp() -> Option<tempfile::TempDir> {
    if !Path::new("/tmp").is_dir() {
        return None;
    }
    Some(
        tempfile::Builder::new()
            .tempdir_in("/tmp")
            .expect("tempdir in /tmp"),
    )
}

fn in_policy_read(p: &SandboxPolicy, path: &Path) -> bool {
    p.allow_read.iter().any(|x| x == path)
}

#[test]
fn test_constants_when_inspected_should_match_contract() {
    assert_eq!(
        HOME_READ_DIRS,
        &[
            ".cache",
            ".cargo",
            ".rustup",
            ".gitconfig",
            ".config/git/config",
            ".config/git/ignore",
            ".config/git/attributes"
        ]
    );
    assert_eq!(EXTRA_READ_ENV, "HONE_SANDBOX_EXTRA_READ");
    assert_eq!(SANDBOX_ENFORCE_ENV, "HONE_SANDBOX_ENFORCE");
    assert_eq!(WRITE_DEVICES, &["/dev/null"]);
}

#[test]
fn test_system_read_roots_when_inspected_should_include_basics() {
    for r in ["/bin", "/usr/bin", "/usr/lib", "/usr/share", "/etc"] {
        assert!(SYSTEM_READ_ROOTS.contains(&r), "missing {r}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_system_read_roots_when_macos_should_be_exact_homebrew_subdirs() {
    assert_eq!(
        SYSTEM_READ_ROOTS,
        &[
            "/bin",
            "/usr/bin",
            "/usr/lib",
            "/usr/share",
            "/etc",
            "/usr/local/bin",
            "/usr/local/lib",
            "/usr/local/opt",
            "/usr/local/Cellar",
            "/usr/local/share",
            "/usr/local/include",
            "/opt/homebrew/bin",
            "/opt/homebrew/lib",
            "/opt/homebrew/opt",
            "/opt/homebrew/Cellar",
            "/opt/homebrew/share",
            "/opt/homebrew/include",
            "/opt/homebrew/etc/gitconfig",
            "/usr/local/etc/gitconfig",
        ]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_system_read_roots_when_macos_should_not_expose_whole_opt_or_usr_local_or_var_etc() {
    assert!(!SYSTEM_READ_ROOTS.contains(&"/opt"));
    assert!(!SYSTEM_READ_ROOTS.contains(&"/usr/local"));
    let allowed_files = ["/opt/homebrew/etc/gitconfig", "/usr/local/etc/gitconfig"];
    for r in SYSTEM_READ_ROOTS {
        for var in ["/opt/homebrew/var", "/usr/local/var"] {
            assert!(!Path::new(r).starts_with(var), "{r} lies under {var}");
        }
        for etc in ["/opt/homebrew/etc", "/usr/local/etc"] {
            assert!(
                !Path::new(r).starts_with(etc) || allowed_files.contains(r),
                "{r} lies under {etc} and is not an allowed gitconfig file"
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn test_system_read_roots_when_not_macos_should_be_exact_linux_set() {
    assert_eq!(
        SYSTEM_READ_ROOTS,
        &[
            "/bin",
            "/usr/bin",
            "/usr/lib",
            "/usr/lib64",
            "/lib",
            "/lib64",
            "/usr/share",
            "/etc",
            "/opt",
            "/usr/local",
        ]
    );
}

#[test]
fn test_sandbox_enforced_when_landlock_should_require_env_exactly_one() {
    assert!(sandbox_enforced(SandboxPlatform::Landlock, Some("1")));
    assert!(!sandbox_enforced(SandboxPlatform::Landlock, None));
    for v in ["0", "", "true", "yes", " 1"] {
        assert!(
            !sandbox_enforced(SandboxPlatform::Landlock, Some(v)),
            "{v:?}"
        );
    }
}

#[test]
fn test_sandbox_enforced_when_other_platforms_should_always_be_true() {
    for plat in [
        SandboxPlatform::Seatbelt,
        SandboxPlatform::Container,
        SandboxPlatform::Null,
    ] {
        for env in [None, Some("0"), Some("1"), Some("")] {
            assert!(sandbox_enforced(plat, env), "{plat:?} {env:?}");
        }
    }
}

#[test]
fn test_policy_when_built_should_be_autonomous() {
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[]).unwrap();
    assert_eq!(p.mode, SandboxMode::Autonomous);
}

#[test]
fn test_policy_when_system_roots_exist_should_list_them_in_constant_order() {
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[]).unwrap();
    let expected: Vec<PathBuf> = SYSTEM_READ_ROOTS
        .iter()
        .map(PathBuf::from)
        .filter(|x| x.exists())
        .collect();
    assert_eq!(p.allow_read, expected);
}

#[test]
fn test_policy_when_built_should_write_working_dir_then_tmp_then_devices() {
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[]).unwrap();
    let mut expected = vec![wd.path().to_path_buf()];
    if Path::new("/tmp").exists() {
        expected.push(PathBuf::from("/tmp"));
    }
    for d in WRITE_DEVICES {
        if Path::new(d).exists() {
            expected.push(PathBuf::from(d));
        }
    }
    assert_eq!(p.allow_write, expected);
}

#[test]
fn test_policy_when_dev_null_exists_should_be_writable() {
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[]).unwrap();
    assert_eq!(
        p.allow_write.iter().any(|x| x == Path::new("/dev/null")),
        Path::new("/dev/null").exists()
    );
}

#[test]
fn test_policy_when_home_has_gitconfig_file_should_include_it() {
    let wd = tmp();
    let home = tmp();
    let f = home.path().join(".gitconfig");
    std::fs::write(&f, "[user]\n").unwrap();
    let p = shell_sandbox_policy(wd.path(), Some(home.path()), &[]).unwrap();
    assert!(in_policy_read(&p, &f));
}

#[test]
fn test_policy_when_home_has_config_git_files_should_list_config_not_dir_or_credentials() {
    let wd = tmp();
    let home = tmp();
    let d = home.path().join(".config").join("git");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("config"), "[user]\n").unwrap();
    std::fs::write(d.join("credentials"), "https://u:secret@example.com\n").unwrap();
    let p = shell_sandbox_policy(wd.path(), Some(home.path()), &[]).unwrap();
    assert!(in_policy_read(&p, &d.join("config")));
    assert!(
        !in_policy_read(&p, &d),
        ".config/git dir must not be listed"
    );
    assert!(!in_policy_read(&p, &d.join("credentials")));
    assert!(!in_policy_read(&p, &home.path().join(".config")));
}

#[test]
fn test_policy_when_extra_is_slash_should_drop_it() {
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[PathBuf::from("/")]).unwrap();
    assert!(!in_policy_read(&p, Path::new("/")));
}

#[test]
fn test_policy_when_extra_equals_home_should_drop_it() {
    let wd = tmp();
    let home = ext();
    let p =
        shell_sandbox_policy(wd.path(), Some(home.path()), &[home.path().to_path_buf()]).unwrap();
    assert!(!in_policy_read(&p, home.path()));
}

#[test]
fn test_policy_when_extra_is_ancestor_of_home_should_drop_it() {
    let wd = tmp();
    let base = ext();
    let home = base.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let p = shell_sandbox_policy(wd.path(), Some(&home), &[base.path().to_path_buf()]).unwrap();
    assert!(!in_policy_read(&p, base.path()));
}

#[test]
fn test_policy_when_extra_is_symlink_to_home_should_drop_it() {
    let wd = tmp();
    let home = ext();
    let holder = ext();
    let link = holder.path().join("toolchain");
    std::os::unix::fs::symlink(home.path(), &link).unwrap();
    let p =
        shell_sandbox_policy(wd.path(), Some(home.path()), std::slice::from_ref(&link)).unwrap();
    assert!(!in_policy_read(&p, &link));
}

#[test]
fn test_policy_when_extra_under_working_dir_should_drop_it() {
    let wd = tmp();
    let sub = wd.path().join("tools");
    std::fs::create_dir(&sub).unwrap();
    let p = shell_sandbox_policy(wd.path(), None, std::slice::from_ref(&sub)).unwrap();
    assert!(!in_policy_read(&p, &sub));
}

#[test]
fn test_policy_when_extra_is_symlink_under_working_dir_to_home_should_drop_it() {
    // Agent swaps an extra root under a writable path for a symlink to $HOME.
    let wd = tmp();
    let home = ext();
    let link = wd.path().join("opt-link");
    std::os::unix::fs::symlink(home.path(), &link).unwrap();
    let p =
        shell_sandbox_policy(wd.path(), Some(home.path()), std::slice::from_ref(&link)).unwrap();
    assert!(!in_policy_read(&p, &link));
    assert!(!in_policy_read(&p, home.path()));
}

#[test]
fn test_policy_when_extra_under_slash_tmp_should_drop_it() {
    let Some(t) = tmp_under_slash_tmp() else {
        eprintln!("skipping: /tmp not present");
        return;
    };
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[t.path().to_path_buf()]).unwrap();
    assert!(!in_policy_read(&p, t.path()));
}

#[test]
fn test_policy_when_extra_is_symlink_under_slash_tmp_should_drop_it() {
    let Some(t) = tmp_under_slash_tmp() else {
        eprintln!("skipping: /tmp not present");
        return;
    };
    let target = ext();
    let link = t.path().join("link");
    std::os::unix::fs::symlink(target.path(), &link).unwrap();
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, std::slice::from_ref(&link)).unwrap();
    assert!(!in_policy_read(&p, &link));
}

#[test]
fn test_policy_when_working_dir_is_under_slash_tmp_and_extra_outside_should_keep_extra() {
    let Some(wd) = tmp_under_slash_tmp() else {
        eprintln!("skipping: /tmp not present");
        return;
    };
    let extra = ext();
    let p = shell_sandbox_policy(wd.path(), None, &[extra.path().to_path_buf()]).unwrap();
    assert!(in_policy_read(&p, extra.path()));
}

#[test]
fn test_policy_when_home_has_no_cache_dirs_should_add_no_home_entries() {
    let wd = tmp();
    let home = tmp();
    let p = shell_sandbox_policy(wd.path(), Some(home.path()), &[]).unwrap();
    assert!(!p.allow_read.iter().any(|x| x.starts_with(home.path())));
}

#[test]
fn test_policy_when_only_cargo_exists_should_add_only_cargo() {
    let wd = tmp();
    let home = tmp();
    std::fs::create_dir(home.path().join(".cargo")).unwrap();
    let p = shell_sandbox_policy(wd.path(), Some(home.path()), &[]).unwrap();
    let under_home: Vec<_> = p
        .allow_read
        .iter()
        .filter(|x| x.starts_with(home.path()))
        .collect();
    assert_eq!(under_home, vec![&home.path().join(".cargo")]);
}

#[test]
fn test_policy_when_all_home_dirs_exist_should_add_in_constant_order() {
    let wd = tmp();
    let home = tmp();
    for d in HOME_READ_DIRS {
        let path = home.path().join(d);
        if *d == ".gitconfig" {
            std::fs::write(&path, "[user]\n").unwrap();
        } else {
            std::fs::create_dir_all(&path).unwrap();
        }
    }
    let p = shell_sandbox_policy(wd.path(), Some(home.path()), &[]).unwrap();
    let under_home: Vec<PathBuf> = p
        .allow_read
        .iter()
        .filter(|x| x.starts_with(home.path()))
        .cloned()
        .collect();
    let expected: Vec<PathBuf> = HOME_READ_DIRS.iter().map(|d| home.path().join(d)).collect();
    assert_eq!(under_home, expected);
}

#[test]
fn test_policy_when_extra_read_missing_should_drop_it() {
    let wd = tmp();
    let base = tmp();
    let missing = base.path().join("nope");
    let p = shell_sandbox_policy(wd.path(), None, std::slice::from_ref(&missing)).unwrap();
    assert!(!p.allow_read.contains(&missing));
}

#[test]
fn test_policy_when_extra_read_exists_should_include_it() {
    let wd = tmp();
    let extra = ext();
    let p = shell_sandbox_policy(wd.path(), None, &[extra.path().to_path_buf()]).unwrap();
    assert!(p.allow_read.contains(&extra.path().to_path_buf()));
}

#[test]
fn test_policy_when_extra_read_relative_should_drop_it() {
    let wd = tmp();
    // "." exists relative to cwd but is not absolute.
    let p = shell_sandbox_policy(wd.path(), None, &[PathBuf::from(".")]).unwrap();
    assert!(!p.allow_read.contains(&PathBuf::from(".")));
}

#[test]
fn test_policy_when_swept_should_contain_no_nonexistent_paths_except_working_dir() {
    let wd = tmp();
    let home = tmp();
    std::fs::create_dir(home.path().join(".rustup")).unwrap();
    let missing = home.path().join("missing-extra");
    let present = ext();
    let p = shell_sandbox_policy(
        wd.path(),
        Some(home.path()),
        &[missing, present.path().to_path_buf(), PathBuf::from("rel")],
    )
    .unwrap();
    for path in p.allow_read.iter().chain(p.allow_write.iter()) {
        assert!(
            path == wd.path() || path.exists(),
            "nonexistent path in policy: {}",
            path.display()
        );
    }
}

#[test]
fn test_policy_when_working_dir_missing_should_still_list_it_for_write() {
    let base = tmp();
    let wd = base.path().join("not-created");
    let p = shell_sandbox_policy(&wd, None, &[]).unwrap();
    assert_eq!(p.allow_write.first(), Some(&wd));
}

#[test]
fn test_policy_when_home_dirs_under_working_dir_should_exclude_them() {
    let home = tmp();
    std::fs::create_dir(home.path().join(".cache")).unwrap();
    std::fs::create_dir(home.path().join(".cargo")).unwrap();
    let p = shell_sandbox_policy(home.path(), Some(home.path()), &[]).unwrap();
    assert!(!p.allow_read.iter().any(|x| x.starts_with(home.path())));
}

#[test]
fn test_policy_when_extras_duplicated_should_dedup() {
    let wd = tmp();
    let extra = ext();
    let e = extra.path().to_path_buf();
    let p = shell_sandbox_policy(wd.path(), None, &[e.clone(), e.clone()]).unwrap();
    assert_eq!(p.allow_read.iter().filter(|x| **x == e).count(), 1);
}

#[test]
fn test_policy_when_extra_duplicates_system_root_should_dedup() {
    let wd = tmp();
    // /etc exists on macOS and Linux.
    let p = shell_sandbox_policy(wd.path(), None, &[PathBuf::from("/etc")]).unwrap();
    assert_eq!(
        p.allow_read
            .iter()
            .filter(|x| **x == Path::new("/etc"))
            .count(),
        1
    );
}

#[test]
fn test_policy_when_all_sources_present_should_order_system_home_extra() {
    let wd = tmp();
    let home = tmp();
    std::fs::create_dir(home.path().join(".cache")).unwrap();
    let extra = ext();
    let p =
        shell_sandbox_policy(wd.path(), Some(home.path()), &[extra.path().to_path_buf()]).unwrap();
    let pos = |needle: &Path| p.allow_read.iter().position(|x| x == needle).unwrap();
    let sys_last = p
        .allow_read
        .iter()
        .rposition(|x| SYSTEM_READ_ROOTS.iter().any(|r| x == Path::new(r)))
        .expect("at least /etc exists");
    let h = pos(&home.path().join(".cache"));
    let e = pos(extra.path());
    assert!(sys_last < h, "system roots must precede home dirs");
    assert!(h < e, "home dirs must precede extras");
}

#[test]
fn test_policy_when_home_is_none_should_build() {
    let wd = tmp();
    let p = shell_sandbox_policy(wd.path(), None, &[]);
    assert!(p.is_ok());
}

#[test]
fn test_parse_extra_read_when_mixed_entries_should_trim_and_drop_empty_and_relative() {
    assert_eq!(
        parse_extra_read("/a:: /b :rel:/c/"),
        vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c/")
        ]
    );
}

#[test]
fn test_parse_extra_read_when_empty_should_return_empty() {
    assert!(parse_extra_read("").is_empty());
    assert!(parse_extra_read(" : ::").is_empty());
}

#[cfg(target_os = "linux")]
mod linux_enforcement {
    use super::*;
    use hone_sandbox::{LandlockSandbox, Sandbox};
    use std::process::Command;

    fn landlock_available() -> bool {
        std::fs::read_to_string("/sys/kernel/security/lsm")
            .map(|s| s.contains("landlock"))
            .unwrap_or(false)
    }

    #[test]
    fn test_landlock_when_home_lacks_cache_dirs_should_still_run_command() {
        if !landlock_available() {
            eprintln!("skipping: landlock not enabled in /sys/kernel/security/lsm");
            return;
        }
        let wd = tmp();
        let home = tmp();
        let policy = shell_sandbox_policy(wd.path(), Some(home.path()), &[]).unwrap();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo ok"]).current_dir(wd.path());
        LandlockSandbox.restrict_child(&mut cmd, &policy).unwrap();
        let out = cmd.output().expect("spawn");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }

    #[test]
    fn test_landlock_when_extra_read_dir_given_should_execute_script_in_it() {
        if !landlock_available() {
            eprintln!("skipping: landlock not enabled in /sys/kernel/security/lsm");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let wd = tmp();
        let extra = ext();
        let script = extra.path().join("tool.sh");
        std::fs::write(&script, "#!/bin/sh\necho extra-ok\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let policy = shell_sandbox_policy(wd.path(), None, &[extra.path().to_path_buf()]).unwrap();
        let mut cmd = Command::new("sh");
        cmd.arg(&script).current_dir(wd.path());
        LandlockSandbox.restrict_child(&mut cmd, &policy).unwrap();
        let out = cmd.output().expect("spawn");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "extra-ok");
    }
}

// ---------------------------------------------------------------------------
// ShellTool::call: enforcement opt-in and hook-preserving command construction
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod call_wiring {
    use super::*;
    use hone_tools::{ShellTool, Tool, ToolContext, ToolInput};
    use serde_json::json;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    /// Fake sandbox whose `restrict_child` installs a `pre_exec` hook that
    /// always fails. If the shell tool drops the hook (rebuilding the command
    /// from program+args), the spawn would wrongly succeed.
    struct HookSandbox(SandboxPlatform);

    impl Sandbox for HookSandbox {
        fn apply(&self, _policy: &SandboxPolicy) -> Result<SandboxHandle, SandboxError> {
            Err(SandboxError::Unavailable {
                platform: "fake".into(),
            })
        }

        fn restrict_child(
            &self,
            cmd: &mut Command,
            _policy: &SandboxPolicy,
        ) -> Result<(), SandboxError> {
            // SAFETY: the closure only constructs an error value.
            unsafe {
                cmd.pre_exec(|| Err(std::io::Error::other("hook ran")));
            }
            Ok(())
        }

        fn platform(&self) -> SandboxPlatform {
            self.0
        }
    }

    fn ctx(dir: &tempfile::TempDir, platform: SandboxPlatform) -> ToolContext {
        ToolContext {
            sandbox: Arc::new(HookSandbox(platform)),
            sandbox_policy: Arc::new(SandboxPolicy::permissive()),
            working_dir: dir.path().to_path_buf(),
        }
    }

    #[tokio::test]
    async fn test_call_when_platform_null_should_preserve_pre_exec_hook_and_fail() {
        let dir = tmp();
        let c = ctx(&dir, SandboxPlatform::Null);
        let res = ShellTool
            .call(ToolInput::new(json!({ "command": "echo ok" })), &c)
            .await;
        match res {
            Err(_) => {}
            Ok(out) => {
                let code = out.value["exit_code"].as_i64().unwrap_or(0);
                assert_ne!(code, 0, "pre_exec hook must have prevented the command");
                assert_ne!(out.value["stdout"], "ok\n");
            }
        }
    }

    #[tokio::test]
    async fn test_call_when_landlock_and_enforce_env_unset_should_not_install_sandbox() {
        // Assumes HONE_SANDBOX_ENFORCE is not set in the test environment.
        let dir = tmp();
        let c = ctx(&dir, SandboxPlatform::Landlock);
        let out = ShellTool
            .call(ToolInput::new(json!({ "command": "echo ok" })), &c)
            .await
            .expect("Landlock is opt-in; hook must not be installed");
        assert_eq!(out.value["exit_code"], 0);
        assert_eq!(out.value["stdout"], "ok\n");
    }
}
