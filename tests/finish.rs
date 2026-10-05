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
fn finish_removes_worktrees_and_session_but_keeps_branches() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "review", "ui", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("finished"));
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert!(!fx.worktree_dir.join("ui-clone").join("feat-1").exists());
    assert!(!fx.worktree_dir.join("acme").join("feat-1").exists());
    assert!(!fx.has_session("feat-1"));
    for (clone, branch) in [("api-clone", "feat-1"), ("ui-clone", "feat-1")] {
        let dir = fx.project_home.join(clone);
        assert_eq!(fx.git(&dir, &["branch", "--list", branch]).trim(), branch);
        assert_eq!(fx.git(&dir, &["worktree", "list"]).lines().count(), 1);
    }
    assert_eq!(
        fx.git(&hub, &["branch", "--list", "feat-1"]).trim(),
        "feat-1",
        "hub branch kept"
    );
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["status"], "finished");
    assert_eq!(
        f["changes"][1]["stage"], "review",
        "stages are left as they were"
    );
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("finished"));
    fx.hub(&hub, &["feature", "add", "api", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("finished"));
}

#[test]
fn finish_checks_every_tree_before_removing_anything() {
    let (fx, hub) = world();
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    std::fs::write(ui_wt.join("scratch.txt"), "x").unwrap();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(20)
        .stderr(predicate::str::contains("scratch.txt"));
    assert!(
        fx.worktree_dir.join("api-clone").join("feat-1").exists(),
        "api (earlier in order) untouched"
    );
    assert!(fx.worktree_dir.join("acme").join("feat-1").exists());
    assert!(fx.has_session("feat-1"));
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "open");

    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    std::fs::remove_file(ui_wt.join("scratch.txt")).unwrap();
    std::fs::write(hub_wt.join("notes.md"), "x").unwrap();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(20)
        .stderr(predicate::str::contains("notes.md"));
    assert!(fx.worktree_dir.join("api-clone").join("feat-1").exists());

    fx.hub(
        &hub,
        &["feature", "finish", "--force", "--feature", "feat-1"],
    )
    .assert()
    .success();
    assert!(!hub_wt.exists());
    assert!(!fx.has_session("feat-1"));
}

#[test]
fn finish_completes_on_rerun_after_a_failed_write() {
    let (fx, hub) = world();
    fx.block_feature_writes(&hub);
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Permission denied"));
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert!(!fx.has_session("feat-1"));
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["status"],
        "open",
        "previous record kept"
    );
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    fx.allow_feature_writes(&hub);
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
}

#[test]
fn finish_needs_no_force_after_copied_local_config() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.exclude_locally(&api, &[".env"]);
    std::fs::write(api.join(".env"), "K=v\n").unwrap();
    fx.hub(&hub, &["feature", "start", "feat-2", "--repo", "api"])
        .assert()
        .success();
    assert!(
        fx.worktree_dir
            .join("api-clone")
            .join("feat-2")
            .join(".env")
            .exists()
    );
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-2"])
        .assert()
        .success();
}

#[test]
fn finish_keeps_declined_adopted_worktrees_and_continues() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "merged", "ui", "--feature", "feat-1"])
        .assert()
        .success();
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
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("kept adopted worktree"));
    assert!(review.exists(), "adopted worktree left in place");
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
}

#[test]
fn finish_records_the_finish_when_the_hub_worktree_cannot_be_deleted() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.block_deletion(&hub, &hub_wt);
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("directory remains"))
        .stdout(predicate::str::contains(hub_wt.display().to_string()));
    assert!(hub_wt.exists());
    assert_eq!(fx.git(&hub, &["worktree", "list"]).lines().count(), 1);
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
    fx.unblock_deletion(&hub_wt);
}

#[test]
fn finish_kills_the_session_then_refuses_live_processes_before_removing_anything() {
    let (fx, hub) = world();
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    let procs = format!("4242\tnode\t{}", ui_wt.display());
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .env("FAKE_LSOF_CWDS", &procs)
        .assert()
        .code(21)
        .stderr(predicate::str::contains("4242"));
    assert!(
        !fx.has_session("feat-1"),
        "the session's own shells go first so they cannot block removal"
    );
    assert!(
        fx.worktree_dir.join("api-clone").join("feat-1").exists(),
        "api (earlier in order) untouched"
    );
    assert!(ui_wt.exists());
    assert!(fx.worktree_dir.join("acme").join("feat-1").exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "open");
    fx.hub(
        &hub,
        &["feature", "finish", "--force", "--feature", "feat-1"],
    )
    .env("FAKE_LSOF_CWDS", &procs)
    .assert()
    .success();
    assert!(!ui_wt.exists());
}

#[test]
fn finish_refuses_to_run_inside_the_session_it_would_kill() {
    let (fx, hub) = world();
    // Killing the session would hang up this very process, so nothing is touched.
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .env("TMUX", "/tmp/tmux-501/default,123,4")
        .env("FAKE_TMUX_CURRENT", "feat-1")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("inside tmux session feat-1"))
        .stderr(predicate::str::contains(
            "hub feature finish --feature feat-1",
        ));
    assert!(fx.has_session("feat-1"));
    assert!(fx.worktree_dir.join("acme").join("feat-1").exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "open");
    // From a different session it proceeds as usual.
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .env("TMUX", "/tmp/tmux-501/default,123,0")
        .env("FAKE_TMUX_CURRENT", "acme-main")
        .assert()
        .success();
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
}

#[test]
fn finish_ignores_its_own_process_tree_when_checking_for_live_processes() {
    let (fx, hub) = world();
    // Run from the hub worktree, as the feature inference invites: the
    // calling shell, hub itself and its lsof all sit in the tree about to go.
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(&hub_wt, &["feature", "finish"])
        .env("FAKE_LSOF_SELF", "1")
        .assert()
        .success();
    assert!(!hub_wt.exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
}

#[test]
fn finish_still_refuses_a_foreign_process_alongside_its_own() {
    let (fx, hub) = world();
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    let procs = format!("4242\tnode\t{}", ui_wt.display());
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .env("FAKE_LSOF_SELF", "1")
        .env("FAKE_LSOF_CWDS", &procs)
        .assert()
        .code(21)
        .stderr(predicate::str::contains("4242"));
    assert!(ui_wt.exists());
}
