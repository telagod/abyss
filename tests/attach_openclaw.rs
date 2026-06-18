//! Integration test for the OpenClaw `attach` downgrade.
//!
//! v0.5.23 deliberately disables hook injection for OpenClaw because
//! OpenClaw uses a per-pack install layout (see the sister `code-abyss`
//! package's `bin/adapters/openclaw.js`). The right migration is
//! `npx code-abyss -t openclaw --with-abyss`. This test pins the
//! downgrade so it can't silently flip back to writing the wrong file.

use code_abyss::attach::openclaw;

#[test]
fn install_at_is_a_clear_error_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".openclaw/config.toml");

    let err = openclaw::install_at(&path).expect_err("openclaw install must fail loudly");
    let msg = format!("{err}");

    assert!(
        msg.contains("per-pack install layout"),
        "error message must explain why we refuse: {msg}"
    );
    assert!(
        msg.contains("npx code-abyss"),
        "error message must point users at the working alternative: {msg}"
    );

    // CRITICAL: no settings file is written. Writing a file OpenClaw
    // doesn't read would be worse than no-op — silent failure.
    assert!(
        !path.exists(),
        "openclaw downgrade must NOT create {}",
        path.display()
    );
}

#[test]
fn install_returns_error_in_home_and_local_mode() {
    assert!(openclaw::install(true).is_err());
    assert!(openclaw::install(false).is_err());
}

#[test]
fn already_installed_is_always_false() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    assert!(!openclaw::already_installed(&path));
    // Even with a hook-shaped file, the downgrade must NOT claim
    // "already present" — the file is irrelevant to OpenClaw.
    std::fs::write(
        &path,
        "[hooks.PreToolUse]\ncommand = \"abyss hook pre-edit\"\n",
    )
    .unwrap();
    assert!(!openclaw::already_installed(&path));
}

#[test]
fn settings_path_local_is_cwd_relative() {
    let p = openclaw::settings_path(true).unwrap();
    assert!(p.is_absolute());
    let tail: std::path::PathBuf = p
        .iter()
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .iter()
        .rev()
        .collect();
    assert_eq!(
        tail,
        std::path::PathBuf::from(".openclaw").join("config.toml")
    );
}
