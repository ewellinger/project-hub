mod common;

use common::Fixture;
use predicates::prelude::*;

fn world() -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(
        &hub,
        &[
            "feature", "start", "feat-1", "--repo", "api", "--repo", "ui",
        ],
    )
    .assert()
    .success();
    (fx, hub)
}

#[test]
fn review_records_stage_url_and_origin_seen() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "review", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("not on origin"));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["stage"], "review");
    assert_eq!(f["changes"][0]["review_url"], serde_json::Value::Null);
    assert_eq!(f["changes"][0]["origin_seen"], false);

    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.commit_file(&wt, "work.txt", "work");
    fx.git(&wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.hub(
        &hub,
        &[
            "feature",
            "review",
            "api",
            "https://gitlab.example/mr/1",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["review_url"], "https://gitlab.example/mr/1");
    assert_eq!(f["changes"][0]["origin_seen"], true);
    fx.hub(&hub, &["feature", "review", "nope", "--feature", "feat-1"])
        .assert()
        .code(1);
}

#[test]
fn merged_closes_a_hub_owned_change() {
    let (fx, hub) = world();
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed worktree"));
    assert!(!wt.exists());
    assert_eq!(
        fx.git(&api, &["branch", "--list", "feat-1"]).trim(),
        "feat-1",
        "branch kept"
    );
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 1);
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["stage"], "merged");
    assert!(
        f["changes"][0]["merged_at"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    let windows = fx.windows("feat-1");
    assert_eq!(windows[2].0, "api");
    assert_eq!(
        windows[2].1,
        api.display().to_string(),
        "window re-pointed at the main clone"
    );
    let ws_path = fx
        .worktree_dir
        .join("acme")
        .join("feat-1")
        .join("acme.code-workspace");
    let ws: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ws_path).unwrap()).unwrap();
    assert_eq!(ws["folders"].as_array().unwrap().len(), 2, "hub + ui only");
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no open change"));
}

#[test]
fn merged_completes_on_rerun_after_a_failed_write() {
    let (fx, hub) = world();
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.block_feature_writes(&hub);
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Permission denied"));
    assert!(!wt.exists(), "the removal is not undone");
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["stage"],
        "working",
        "previous record kept"
    );
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    fx.allow_feature_writes(&hub);
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already gone"));
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["stage"],
        "merged"
    );
}

#[test]
fn merged_refuses_dirty_without_force() {
    let (fx, hub) = world();
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    std::fs::write(wt.join("scratch.txt"), "x").unwrap();
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .code(20)
        .stderr(predicate::str::contains("scratch.txt"));
    assert!(wt.exists());
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["stage"],
        "working"
    );
    let windows = fx.windows("feat-1");
    assert_eq!(windows[2].0, "api");
    assert_eq!(
        windows[2].1,
        wt.display().to_string(),
        "a refused merge leaves the role's window, and whatever runs in it, alone"
    );
    assert!(
        !fx.tmux_log().contains("respawn-pane"),
        "no shell was killed before the dirty check"
    );
    fx.hub(
        &hub,
        &["feature", "merged", "api", "--force", "--feature", "feat-1"],
    )
    .assert()
    .success();
    assert!(!wt.exists());
}

#[test]
fn merged_keeps_an_adopted_worktree_unless_forced() {
    let (fx, hub) = world();
    fx.remote_branch("ui-clone", "shared");
    let ui = fx.project_home.join("ui-clone");
    let review = fx.worktree_dir.join("ui-clone").join("ui-review_3");
    fx.git(
        &ui,
        &[
            "worktree",
            "add",
            "-q",
            "--track",
            "-b",
            "shared",
            review.to_str().unwrap(),
            "origin/shared",
        ],
    );
    fx.hub(&hub, &["feature", "merged", "ui", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(
        &hub,
        &[
            "feature",
            "add",
            "ui",
            "--branch",
            "shared",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    fx.hub(&hub, &["feature", "merged", "ui", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("kept"));
    assert!(review.exists());
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][2]["stage"],
        "working"
    );
    fx.hub(
        &hub,
        &["feature", "merged", "ui", "--force", "--feature", "feat-1"],
    )
    .assert()
    .success();
    assert!(!review.exists());
    assert_eq!(
        fx.git(&ui, &["branch", "--list", "shared"]).trim(),
        "shared"
    );
}

#[test]
fn merged_on_a_main_clone_change_removes_nothing() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success();
    let api = fx.project_home.join("api-clone");
    fx.git(&api, &["checkout", "-q", "-b", "feat-1-2"]);
    fx.hub(
        &hub,
        &[
            "feature",
            "add",
            "api",
            "--branch",
            "feat-1-2",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][2]["branch"], "feat-1-2");
    assert_eq!(
        f["changes"][2]["worktree"],
        serde_json::Value::Null,
        "adopted from the main clone"
    );
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("main clone"));
    assert_eq!(
        fx.git(&api, &["symbolic-ref", "--short", "HEAD"]),
        "feat-1-2",
        "main clone untouched"
    );
}

#[test]
fn follow_up_after_merge_gets_a_fresh_worktree_and_window() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "add", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Re-pointed tmux window api"));
    let wt = fx.worktree_dir.join("api-clone").join("feat-1-2");
    assert!(wt.is_dir());
    let windows = fx.windows("feat-1");
    assert_eq!(windows[2].0, "api");
    assert_eq!(windows[2].1, wt.display().to_string());
    assert_eq!(fx.window_names("feat-1"), vec!["hub", "ai", "api", "ui"]);
}

#[test]
fn merged_records_the_merge_when_git_unregisters_but_cannot_delete() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.block_deletion(&api, &wt);
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("directory remains"))
        .stdout(predicate::str::contains(wt.display().to_string()));
    assert!(wt.exists(), "the residue is left for the user to inspect");
    assert_eq!(
        fx.git(&api, &["worktree", "list"]).lines().count(),
        1,
        "git unregistered the worktree"
    );
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["stage"], "merged");
    fx.unblock_deletion(&wt);
}

#[test]
fn merged_leaves_the_record_alone_when_the_worktree_stays_registered() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.git(&api, &["worktree", "lock", wt.to_str().unwrap()]);
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("locked"));
    assert!(wt.exists());
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 2);
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["stage"],
        "working"
    );
}

#[test]
fn merged_refuses_a_worktree_with_a_live_process_unless_forced() {
    let (fx, hub) = world();
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    let procs = format!("4242\tvite\t{}/app", wt.display());
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .env("FAKE_LSOF_CWDS", &procs)
        .assert()
        .code(21)
        .stderr(predicate::str::contains("4242"))
        .stderr(predicate::str::contains("vite"));
    assert!(wt.exists());
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["stage"],
        "working"
    );
    // A process elsewhere, even under a sibling with a shared prefix, is not in use.
    let elsewhere = format!("4243\tvite\t{}-other", wt.display());
    fx.hub(&hub, &["feature", "merged", "ui", "--feature", "feat-1"])
        .env("FAKE_LSOF_CWDS", &elsewhere)
        .assert()
        .success();
    fx.hub(
        &hub,
        &["feature", "merged", "api", "--force", "--feature", "feat-1"],
    )
    .env("FAKE_LSOF_CWDS", &procs)
    .assert()
    .success();
    assert!(!wt.exists());
}

#[test]
fn merged_repoints_the_tmux_window_before_checking_for_live_processes() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    let procs = format!("4242\tvite\t{}", wt.display());
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .env("FAKE_LSOF_CWDS", &procs)
        .assert()
        .code(21);
    let windows = fx.windows("feat-1");
    assert_eq!(windows[2].0, "api");
    assert_eq!(
        windows[2].1,
        api.display().to_string(),
        "the hub's own window no longer sits in the worktree"
    );
}
