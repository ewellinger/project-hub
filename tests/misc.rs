mod common;

use common::Fixture;
use hub::hub::Hub;
use predicates::prelude::*;

fn world() -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
    (fx, hub)
}

#[test]
fn tmux_recreates_a_missing_session() {
    let (fx, hub) = world();
    std::fs::remove_dir_all(fx.tmux_state.join("feat-1")).unwrap();
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created tmux session feat-1"))
        .stdout(predicate::str::contains(
            "Attach with: tmux attach -t feat-1",
        ));
    assert_eq!(fx.window_names("feat-1"), vec!["hub", "ai", "api"]);
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .assert()
        .success();
    assert_eq!(
        fx.window_names("feat-1"),
        vec!["hub", "ai", "api"],
        "idempotent"
    );
}

#[test]
fn tmux_adds_missing_fixed_windows_to_an_existing_session() {
    let (fx, hub) = world();
    std::fs::remove_dir_all(fx.tmux_state.join("feat-1")).unwrap();
    fx.seed_session("feat-1", &[("scratch", "/x")]);
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Added tmux window hub"));
    assert_eq!(
        fx.window_names("feat-1"),
        vec!["scratch", "hub", "ai", "api"],
        "existing windows stay where they are; missing ones are appended"
    );
}

#[test]
fn finished_features_are_read_only() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    let head = fx.git(&hub, &["rev-parse", "HEAD"]);
    for args in [
        vec!["feature", "add", "api", "--feature", "feat-1"],
        vec!["feature", "review", "api", "--feature", "feat-1"],
        vec!["feature", "merged", "api", "--feature", "feat-1"],
        vec!["feature", "finish", "--feature", "feat-1"],
        vec!["tmux", "--feature", "feat-1"],
        vec!["open", "--feature", "feat-1"],
        vec!["sync", "--feature", "feat-1"],
    ] {
        fx.hub(&hub, &args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("finished"));
    }
    assert!(!fx.has_session("feat-1"), "nothing recreated the session");
    assert!(!fx.worktree_dir.join("acme").join("feat-1").exists());
    assert_eq!(fx.git(&hub, &["rev-parse", "HEAD"]), head, "no new commits");
    fx.hub(&hub, &["feature", "list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("feat-1"));
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success();
}

#[test]
fn argument_errors_exit_2_and_help_exits_0() {
    let (fx, hub) = world();
    fx.hub(&hub, &["bogus"]).assert().code(2);
    fx.hub(&hub, &["feature", "start"]).assert().code(2);
    fx.hub(&hub, &["repo", "add", "api", "api-clone", "--nope"])
        .assert()
        .code(2);
    fx.hub(&hub, &["--help"]).assert().success();
    fx.hub(&hub, &["feature", "start", "--help"])
        .assert()
        .success();
}

#[test]
fn open_launches_code_on_the_workspace() {
    let (fx, _hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(&hub_wt, &["open"]).assert().success();
    let log = std::fs::read_to_string(&fx.code_log).unwrap();
    assert_eq!(
        log.trim(),
        hub_wt.join("acme.code-workspace").display().to_string()
    );
}

#[test]
fn open_uses_the_editor_from_the_config_file() {
    let (fx, _hub) = world();
    fx.write_config("editor = \"code --new-window\"\n");
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(&hub_wt, &["open"]).assert().success();
    let log = std::fs::read_to_string(&fx.code_log).unwrap();
    assert_eq!(
        log.trim(),
        format!(
            "--new-window {}",
            hub_wt.join("acme.code-workspace").display()
        )
    );
    fx.write_config("editor = \"no-such-editor-here\"\n");
    fx.hub(&hub_wt, &["open"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("running no-such-editor-here"));
}

#[test]
fn open_from_the_hub_opens_the_base_workspace() {
    let (fx, hub) = world();
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "ui", "ui-clone");

    fx.hub(&hub, &["open"]).assert().success();

    let workspace = hub.join("acme.code-workspace");
    let log = std::fs::read_to_string(&fx.code_log).unwrap();
    assert_eq!(log.trim(), workspace.display().to_string());
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(workspace).unwrap()).unwrap();
    assert_eq!(
        json["folders"],
        serde_json::json!([
            {"name": "acme", "path": "."},
            {"name": "api", "path": "../api-clone"},
            {"name": "ui", "path": "../ui-clone"}
        ])
    );
}

#[test]
fn sync_regenerates_the_workspace_and_preserves_settings() {
    let (fx, hub) = world();
    let ws = fx
        .worktree_dir
        .join("acme")
        .join("feat-1")
        .join("acme.code-workspace");
    std::fs::write(&ws, r#"{"folders":[],"settings":{"editor.tabSize":2}}"#).unwrap();
    let head = fx.git(&hub, &["rev-parse", "HEAD"]);
    fx.hub(&hub, &["sync", "--feature", "feat-1"])
        .assert()
        .success();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ws).unwrap()).unwrap();
    assert_eq!(json["settings"]["editor.tabSize"], 2);
    assert_eq!(json["folders"].as_array().unwrap().len(), 2);
    let before = std::fs::read_to_string(&ws).unwrap();
    fx.hub(&hub, &["sync", "--feature", "feat-1"])
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&ws).unwrap(), before);
    assert_eq!(
        fx.git(&hub, &["rev-parse", "HEAD"]),
        head,
        "sync does not commit"
    );
}

#[test]
fn repo_remove_is_refused_while_a_feature_uses_the_role() {
    let (fx, hub) = world();
    fx.hub(&hub, &["repo", "remove", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("feat-1").and(predicate::str::contains("open change")));
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "remove", "api"]).assert().success();
    // History for the removed role stays readable: the change carries its clone.
    fx.hub(&hub, &["feature", "list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api:working"));
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api").and(predicate::str::contains("feat-1")));
}

#[test]
fn a_second_mutating_command_is_refused_while_the_lock_is_held() {
    let (fx, hub) = world();
    let locked = Hub::locate(&hub, fx.env()).unwrap();
    locked
        .with_lock(|_| {
            fx.hub(&hub, &["feature", "start", "feat-2"])
                .assert()
                .code(1)
                .stderr(predicate::str::contains("another hub command"));
            Ok(())
        })
        .unwrap();
    assert!(!hub.join(".git/hub/features/feat-2.json").exists());
    fx.hub(&hub, &["feature", "start", "feat-2"])
        .assert()
        .success();
}

#[test]
fn bare_hub_without_a_terminal_prints_usage_and_exits_2() {
    let (fx, hub) = world();
    fx.hub(&hub, &[])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Usage: hub"))
        .stderr(predicate::str::contains("requires a subcommand"));
}

#[test]
fn feature_records_live_under_git_hub_and_never_on_main() {
    let (fx, hub) = world();
    let record = hub.join(".git/hub/features/feat-1.json");
    assert!(record.is_file(), "{}", record.display());
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo add api",
        "starting a feature commits nothing"
    );
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    assert!(!hub_wt.join("features").exists(), "nothing on the branch");
    let from_main = fx
        .hub(&hub, &["feature", "list", "--json"])
        .output()
        .unwrap();
    let from_wt = fx
        .hub(&hub_wt, &["feature", "list", "--json"])
        .output()
        .unwrap();
    assert!(from_main.status.success() && from_wt.status.success());
    assert_eq!(
        from_main.stdout, from_wt.stdout,
        "a feature worktree reads the same store as the main clone"
    );
}

#[test]
fn a_hub_with_records_on_main_is_refused_until_moved_by_hand() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    let legacy = hub.join("features");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("old.json"),
        r#"{"name":"old","checkout":"old","status":"finished","created":"2026-01-01T00:00:00Z","changes":[]}"#,
    )
    .unwrap();
    fx.git(&hub, &["add", "features"]);
    fx.git(&hub, &["commit", "-q", "-m", "legacy record"]);
    fx.hub(&hub, &["feature", "list", "--all"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::contains("features/ on main")
                .and(predicate::str::contains("git rm -r -q features")),
        );

    // Unrelated staged work must survive the move.
    std::fs::write(hub.join("notes.txt"), "n\n").unwrap();
    fx.git(&hub, &["add", "notes.txt"]);

    // The steps the message prints.
    std::fs::create_dir_all(hub.join(".git/hub/features")).unwrap();
    std::fs::rename(
        legacy.join("old.json"),
        hub.join(".git/hub/features/old.json"),
    )
    .unwrap();
    fx.git(&hub, &["rm", "-r", "-q", "features"]);
    fx.git(
        &hub,
        &[
            "commit",
            "-q",
            "-m",
            "hub: move feature state out of git",
            "--",
            "features",
        ],
    );
    fx.hub(&hub, &["feature", "list", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("old"));
    assert_eq!(
        fx.git(&hub, &["diff", "--cached", "--name-only"]),
        "notes.txt",
        "unrelated staged work survives the move"
    );
    assert_eq!(
        fx.git(&hub, &["show", "--name-only", "--format=", "HEAD"]),
        "features/old.json"
    );
}
