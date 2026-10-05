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
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "ui"])
        .assert()
        .success();
    (fx, hub)
}

#[test]
fn adds_a_role_from_inside_the_hub_worktree() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(&hub_wt, &["feature", "add", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: feat-1 at"))
        .stdout(predicate::str::contains(
            "Attach with: tmux attach -t feat-1",
        ));
    let f = fx.feature_json(&hub, "feat-1");
    let changes = f["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1]["role"], "api");
    assert_eq!(changes[1]["stage"], "working");
    assert_eq!(
        changes[1]["base"], "master",
        "the manifest base is recorded explicitly"
    );
    assert_eq!(
        fx.window_names("feat-1"),
        vec!["hub", "ai", "ui", "api"],
        "appended after existing role windows"
    );
    let ws: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hub_wt.join("acme.code-workspace")).unwrap())
            .unwrap();
    let names: Vec<&str> = ws["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["acme", "api", "ui"],
        "workspace lists manifest order"
    );
}

#[test]
fn feature_flag_works_from_the_main_clone_and_options_pass_through() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.git(&api, &["branch", "start-here", "origin/master"]);
    fx.hub(
        &hub,
        &[
            "feature",
            "add",
            "api",
            "--feature",
            "feat-1",
            "--branch",
            "feature/x",
            "--from",
            "start-here",
            "--worktree",
            "wt-x",
        ],
    )
    .assert()
    .success();
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][1]["branch"], "feature/x");
    assert_eq!(f["changes"][1]["worktree"]["name"], "wt-x");
    assert!(fx.worktree_dir.join("api-clone").join("wt-x").is_dir());
    fx.hub(&hub, &["feature", "add", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("--feature"));
}

#[test]
fn refuses_a_second_open_change_and_rolls_back_on_failure() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "add", "ui", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already has an open change"));

    let collision = fx.worktree_dir.join("api-clone").join("feat-1");
    std::fs::create_dir_all(&collision).unwrap();
    fx.hub(&hub, &["feature", "add", "api", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
    let api = fx.project_home.join("api-clone");
    assert_eq!(
        fx.git(&api, &["branch", "--list", "feat-1"]),
        "",
        "created branch rolled back"
    );
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"].as_array().unwrap().len(), 1);
    assert_eq!(fx.window_names("feat-1"), vec!["hub", "ai", "ui"]);
}

#[test]
fn rolls_back_the_new_change_when_the_record_write_fails() {
    let (fx, hub) = world();
    fx.block_feature_writes(&hub);
    fx.hub(&hub, &["feature", "add", "api", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Permission denied"));
    let api = fx.project_home.join("api-clone");
    assert_eq!(
        fx.git(&api, &["branch", "--list", "feat-1"]),
        "",
        "branch rolled back"
    );
    assert!(
        !fx.worktree_dir.join("api-clone").join("feat-1").exists(),
        "worktree rolled back"
    );
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "previous record kept"
    );
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    assert_eq!(fx.window_names("feat-1"), vec!["hub", "ai", "ui"]);
    fx.allow_feature_writes(&hub);
}

#[test]
fn adopts_a_review_worktree_and_points_the_window_at_it() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feat-1");
    let api = fx.project_home.join("api-clone");
    let review = fx.worktree_dir.join("api-clone").join("api-review_9");
    std::fs::create_dir_all(review.parent().unwrap()).unwrap();
    fx.git(
        &api,
        &[
            "worktree",
            "add",
            "-q",
            "--track",
            "-b",
            "feat-1",
            review.to_str().unwrap(),
            "origin/feat-1",
        ],
    );
    fx.hub(&hub, &["feature", "add", "api", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Adopting worktree"));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][1]["worktree"]["owner"], "adopted");
    assert_eq!(f["changes"][1]["origin_seen"], true);
    let windows = fx.windows("feat-1");
    assert_eq!(windows[3].0, "api");
    assert_eq!(windows[3].1, review.display().to_string());
}

#[test]
fn add_without_a_role_needs_a_terminal() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "add", "--feature", "feat-1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "role is required when not running in a terminal",
        ));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(
        f["changes"].as_array().unwrap().len(),
        1,
        "nothing was added"
    );
}

#[test]
fn base_flag_records_the_integration_branch_and_from_the_start_point() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/canonical");
    let api = fx.project_home.join("api-clone");
    let fork = fx.git(&api, &["rev-parse", "origin/master"]);
    fx.git(&api, &["branch", "start-here", &fork]);
    fx.hub(
        &hub,
        &[
            "feature",
            "add",
            "api",
            "--feature",
            "feat-1",
            "--base",
            "feature/canonical",
            "--from",
            "start-here",
        ],
    )
    .assert()
    .success();
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][1]["base"], "feature/canonical");
    assert_eq!(f["changes"][1]["base_sha"], fork);
    assert_eq!(fx.git(&api, &["rev-parse", "feat-1"]), fork);
}

#[test]
fn base_flag_refuses_a_branch_missing_from_origin_without_partial_state() {
    let (fx, hub) = world();
    fx.hub(
        &hub,
        &[
            "feature",
            "add",
            "api",
            "--feature",
            "feat-1",
            "--base",
            "nope",
        ],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("origin/nope"));
    let api = fx.project_home.join("api-clone");
    assert_eq!(fx.git(&api, &["branch", "--list", "feat-1"]), "");
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fx.window_names("feat-1"), vec!["hub", "ai", "ui"]);
}
