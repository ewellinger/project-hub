mod common;

use std::path::{Path, PathBuf};

use common::Fixture;
use predicates::prelude::*;

/// Hub with api (master) and ui (develop); feat-1 works on ui only, so api
/// is free to freeze.
fn world() -> (Fixture, PathBuf) {
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

fn hub_wt(fx: &Fixture) -> PathBuf {
    fx.worktree_dir.join("acme").join("feat-1")
}

fn frozen_wt(fx: &Fixture) -> PathBuf {
    fx.worktree_dir.join("api-clone").join("feat-1-frozen")
}

fn head(fx: &Fixture, dir: &Path) -> String {
    fx.git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn freeze_pins_the_local_base_tip_in_a_detached_worktree() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    // Local master is ahead of origin/master: the pin must be the local tip.
    let tip = fx.commit_file(&api, "local.txt", "local only");

    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "api: frozen at {}",
            &tip[..7]
        )));

    let wt = frozen_wt(&fx);
    assert_eq!(head(&fx, &wt), tip);
    assert_eq!(
        fx.git(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "HEAD",
        "detached"
    );
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["pins"][0]["role"], "api");
    assert_eq!(f["pins"][0]["base"], "master");
    assert_eq!(f["pins"][0]["sha"], tip);
    assert_eq!(f["pins"][0]["worktree"]["name"], "feat-1-frozen");
    assert_eq!(f["pins"][0]["worktree"]["owner"], "hub");
    assert!(fx.window_names("feat-1").contains(&"api".to_string()));
    let workspace = std::fs::read_to_string(hub_wt(&fx).join("acme.code-workspace")).unwrap();
    assert!(workspace.contains("feat-1-frozen"), "{workspace}");

    // The main clone moving on does not move the pin.
    fx.commit_file(&api, "later.txt", "later");
    assert_eq!(head(&fx, &wt), tip);
}

#[test]
fn freeze_at_must_name_a_commit_on_the_local_base() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let first = head(&fx, &api);
    fx.commit_file(&api, "second.txt", "second");
    fx.git(&api, &["branch", "side", &first]);
    fx.git(&api, &["checkout", "-q", "side"]);
    let side = fx.commit_file(&api, "side.txt", "side");
    fx.git(&api, &["checkout", "-q", "master"]);

    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api", "--at", &side])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not on master"));
    assert!(!frozen_wt(&fx).exists());
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());

    fx.hub(
        &hub_wt(&fx),
        &["feature", "freeze", "api", "--at", &first[..10]],
    )
    .assert()
    .success();
    assert_eq!(head(&fx, &frozen_wt(&fx)), first);
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["sha"], first);
}

#[test]
fn freeze_refuses_repeats_unknown_roles_and_occupied_paths() {
    let (fx, _hub) = world();
    let wt = hub_wt(&fx);
    fx.hub(&wt, &["feature", "freeze", "nope"])
        .assert()
        .failure();

    std::fs::create_dir_all(frozen_wt(&fx)).unwrap();
    fx.hub(&wt, &["feature", "freeze", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    std::fs::remove_dir(frozen_wt(&fx)).unwrap();

    fx.hub(&wt, &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(&wt, &["feature", "freeze", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already frozen"));
}

#[test]
fn freeze_removes_its_worktree_when_the_record_write_fails() {
    let (fx, hub) = world();
    fx.block_feature_writes(&hub);
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Permission denied"));
    fx.allow_feature_writes(&hub);
    assert!(!frozen_wt(&fx).exists());
    let api = fx.project_home.join("api-clone");
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 1);
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());
}

#[test]
fn refreeze_moves_the_pin_and_worktree_to_the_local_tip() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let old = head(&fx, &frozen_wt(&fx));

    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already frozen at the tip"));

    fx.commit_file(&api, "u1.txt", "u1");
    let tip = fx.commit_file(&api, "u2.txt", "u2");
    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "api: refrozen at {}",
            &tip[..7]
        )))
        .stdout(predicate::str::contains(format!("+2 since {}", &old[..7])));
    assert_eq!(head(&fx, &frozen_wt(&fx)), tip);
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["sha"], tip);
}

#[test]
fn refreeze_refuses_a_dirty_frozen_worktree() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let old = head(&fx, &frozen_wt(&fx));
    fx.commit_file(&api, "u1.txt", "u1");
    std::fs::write(frozen_wt(&fx).join("scratch.txt"), "x\n").unwrap();

    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "api"])
        .assert()
        .code(20);
    assert_eq!(head(&fx, &frozen_wt(&fx)), old);
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["sha"], old);
}

#[test]
fn unfreeze_removes_the_worktree_and_points_the_window_at_the_main_clone() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: unfrozen"));
    assert!(!frozen_wt(&fx).exists());
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());
    let api_dir = fx.project_home.join("api-clone");
    assert!(
        fx.windows("feat-1")
            .iter()
            .any(|(n, d)| n == "api" && Path::new(d) == api_dir),
        "{:?}",
        fx.windows("feat-1")
    );
    let workspace = std::fs::read_to_string(hub_wt(&fx).join("acme.code-workspace")).unwrap();
    assert!(!workspace.contains("feat-1-frozen"), "{workspace}");

    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not frozen"));
}

#[test]
fn unfreeze_needs_force_for_a_dirty_worktree() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    std::fs::write(frozen_wt(&fx).join("scratch.txt"), "x\n").unwrap();
    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api"])
        .assert()
        .code(20);
    assert!(frozen_wt(&fx).exists());
    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api", "--force"])
        .assert()
        .success();
    assert!(!frozen_wt(&fx).exists());
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());
}

#[test]
fn refreeze_refuses_a_frozen_worktree_with_commits_off_its_pin() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let pin = head(&fx, &frozen_wt(&fx));
    let work = fx.commit_file(&frozen_wt(&fx), "work.txt", "work on the detached head");
    fx.commit_file(&api, "u1.txt", "u1");

    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("role 'api'"))
        .stderr(predicate::str::contains(&work[..7]))
        .stderr(predicate::str::contains("git -C"));
    assert_eq!(
        head(&fx, &frozen_wt(&fx)),
        work,
        "the commit is still checked out"
    );
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["sha"], pin);
}

#[test]
fn unfreeze_refuses_commits_off_the_pin_unless_forced() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let work = fx.commit_file(&frozen_wt(&fx), "work.txt", "work on the detached head");

    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(&work[..7]))
        .stderr(predicate::str::contains("git -C"));
    assert_eq!(
        head(&fx, &frozen_wt(&fx)),
        work,
        "worktree and commit survive"
    );
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["role"], "api");

    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api", "--force"])
        .assert()
        .success();
    assert!(!frozen_wt(&fx).exists());
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());
}

#[test]
fn refreeze_puts_the_worktree_back_when_the_record_write_fails() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let pin = head(&fx, &frozen_wt(&fx));
    fx.commit_file(&api, "u1.txt", "u1");

    fx.block_feature_writes(&hub);
    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Permission denied"));
    fx.allow_feature_writes(&hub);
    assert_eq!(head(&fx, &frozen_wt(&fx)), pin);
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["sha"], pin);
}

#[test]
fn unfreeze_reports_the_stray_commits_before_the_dirt() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.commit_file(&frozen_wt(&fx), "work.txt", "work on the detached head");
    std::fs::write(frozen_wt(&fx).join("scratch.txt"), "x\n").unwrap();

    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("is off its pin"));
    assert!(frozen_wt(&fx).exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["role"], "api");
}

#[test]
fn status_lists_the_frozen_role_with_its_pin_and_base_movement() {
    let (fx, _hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let pin = head(&fx, &frozen_wt(&fx));
    for n in 1..=3 {
        fx.commit_file(&api, &format!("u{n}.txt"), "upstream");
    }

    let out = fx
        .hub(&hub_wt(&fx), &["status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let row = report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == "api")
        .expect("frozen api row");
    assert_eq!(row["stage"], "frozen");
    assert_eq!(row["branch"], "master");
    assert_eq!(row["frozen_sha"], pin);
    assert_eq!(row["base_moved"], 3);
    assert_eq!(row["drift"], false);
    assert_eq!(
        Path::new(row["path"].as_str().unwrap()),
        frozen_wt(&fx).as_path()
    );

    fx.hub(&hub_wt(&fx), &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("frozen @{}", &pin[..7])))
        .stdout(predicate::str::contains("master +3 since pin"));
}

#[test]
fn status_reports_drift_when_the_frozen_worktree_leaves_its_pin() {
    let (fx, _hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.commit_file(&frozen_wt(&fx), "oops.txt", "commit on the detached head");
    fx.hub(&hub_wt(&fx), &["status"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains("off-pin"));

    fx.git(
        &fx.project_home.join("api-clone"),
        &[
            "worktree",
            "remove",
            "--force",
            frozen_wt(&fx).to_str().unwrap(),
        ],
    );
    fx.hub(&hub_wt(&fx), &["status"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains("missing"));
}

#[test]
fn finish_removes_frozen_worktrees_and_refuses_dirty_ones_without_force() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    std::fs::write(frozen_wt(&fx).join("scratch.txt"), "x\n").unwrap();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(20);
    assert!(frozen_wt(&fx).exists());
    std::fs::remove_file(frozen_wt(&fx).join("scratch.txt")).unwrap();

    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: removed frozen worktree"));
    assert!(!frozen_wt(&fx).exists());
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["status"], "finished");
    assert!(f.get("pins").is_none());
    let api = fx.project_home.join("api-clone");
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn finish_refuses_commits_off_the_pin_unless_forced() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let work = fx.commit_file(&frozen_wt(&fx), "work.txt", "work on the detached head");

    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("role 'api'"))
        .stderr(predicate::str::contains(&work[..7]))
        .stderr(predicate::str::contains("git -C"));
    assert_eq!(
        head(&fx, &frozen_wt(&fx)),
        work,
        "worktree and commit survive"
    );
    assert!(hub_wt(&fx).exists(), "nothing was removed");
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "open");

    fx.hub(
        &hub,
        &["feature", "finish", "--feature", "feat-1", "--force"],
    )
    .assert()
    .success();
    assert!(!frozen_wt(&fx).exists());
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "finished");
}

#[test]
fn finish_reports_stray_commits_before_a_dirty_change_worktree() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.commit_file(&frozen_wt(&fx), "work.txt", "work on the detached head");
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    std::fs::write(ui_wt.join("scratch.txt"), "x\n").unwrap();

    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("is off its pin"));
    assert!(frozen_wt(&fx).exists());
    assert!(ui_wt.exists());
    assert!(hub_wt(&fx).exists(), "nothing was removed");
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "open");
}

#[test]
fn pull_leaves_frozen_worktrees_alone() {
    let (fx, _hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let pin = head(&fx, &frozen_wt(&fx));
    fx.hub(&hub_wt(&fx), &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "api: frozen at {}; not pulled",
            &pin[..7]
        )));
    assert_eq!(head(&fx, &frozen_wt(&fx)), pin);
}

#[test]
fn repo_remove_and_clone_change_refuse_while_a_role_is_frozen() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "remove", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("frozen"));
    fx.make_repo("api2-clone", "master");
    fx.hub(&hub, &["repo", "set", "api", "--clone", "api2-clone"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("frozen"));
}

#[test]
fn repo_rename_moves_the_pin_and_its_window() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(&hub, &["repo", "rename", "api", "backend"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Renamed 0 changes and 1 pins in 1 features",
        ));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["pins"][0]["role"], "backend");
    let names = fx.window_names("feat-1");
    assert!(names.contains(&"backend".to_string()), "{names:?}");
    assert!(!names.contains(&"api".to_string()), "{names:?}");
    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "backend"])
        .assert()
        .success();
}

#[test]
fn repo_rename_records_only_refuses_to_give_a_role_two_checkouts() {
    let (fx, hub) = world();
    // feat-1 now has a frozen api worktree and an open ui change.
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let before = fx.feature_json(&hub, "feat-1");

    // The api pin would land beside the open ui change.
    fx.hub(&hub, &["repo", "rename", "api", "ui", "--records-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "renaming its 'api' pin would give",
        ));
    // The open ui change would land beside the frozen api worktree.
    fx.hub(&hub, &["repo", "rename", "ui", "api", "--records-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "renaming its open 'ui' change would give",
        ));

    assert_eq!(
        fx.feature_json(&hub, "feat-1"),
        before,
        "no record was written"
    );
}

#[test]
fn repo_rename_records_only_refuses_to_give_a_role_two_pins() {
    let (fx, hub) = world();
    fx.make_repo("db-clone", "master");
    fx.add_repo(&hub, "db", "db-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "db"])
        .assert()
        .success();
    let before = fx.feature_json(&hub, "feat-1");

    fx.hub(&hub, &["repo", "rename", "api", "db", "--records-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "renaming its 'api' pin would give",
        ));

    assert_eq!(
        fx.feature_json(&hub, "feat-1"),
        before,
        "no record was written"
    );
}

fn status_row(fx: &Fixture, role: &str) -> serde_json::Value {
    let out = fx
        .hub(&hub_wt(fx), &["status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: serde_json::Value = serde_json::from_slice(&out).unwrap();
    report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == role)
        .cloned()
        .unwrap_or_else(|| panic!("no status row for {role}"))
}

#[test]
fn freeze_on_an_open_change_pins_its_merge_base_with_the_local_base() {
    let (fx, hub) = world();
    let ui = fx.project_home.join("ui-clone");
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    fx.commit_file(&ui_wt, "work.txt", "feature work");
    let fork = fx.git(&ui, &["merge-base", "feat-1", "develop"]);
    // Local develop moves on after the fork; the pin stays at the fork.
    fx.commit_file(&ui, "later.txt", "later on develop");

    fx.hub(&hub_wt(&fx), &["feature", "freeze", "ui"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "ui: frozen at {}",
            &fork[..7]
        )))
        .stdout(predicate::str::contains("feat-1 is 0 behind the pin"));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["pins"][0]["role"], "ui");
    assert_eq!(f["pins"][0]["base"], "develop");
    assert_eq!(f["pins"][0]["sha"], fork);
    assert!(f["pins"][0]["worktree"].is_null(), "no reference worktree");
    assert!(
        !fx.worktree_dir
            .join("ui-clone")
            .join("feat-1-frozen")
            .exists()
    );

    let row = status_row(&fx, "ui");
    assert_eq!(row["stage"], "working");
    assert_eq!(row["frozen_sha"], fork);
    assert_eq!(row["base_behind"], 0);
    assert_eq!(row["base_moved"], 1);
    fx.hub(&hub_wt(&fx), &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("frozen @{}", &fork[..7])))
        .stdout(predicate::str::contains("develop +1 since pin"));
}

#[test]
fn refreeze_on_an_open_change_moves_the_pin_to_the_local_base_and_reports_the_lag() {
    let (fx, hub) = world();
    let ui = fx.project_home.join("ui-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "ui"])
        .assert()
        .success();
    // The pin is already the local tip: still report the lag.
    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "ui"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already frozen at the tip"))
        .stdout(predicate::str::contains("feat-1 is 0 behind the pin"));
    let branch_tip = fx.git(&ui, &["rev-parse", "feat-1"]);
    fx.commit_file(&ui, "d1.txt", "d1");
    let tip = fx.commit_file(&ui, "d2.txt", "d2");

    fx.hub(&hub_wt(&fx), &["feature", "refreeze", "ui"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "ui: refrozen at {}",
            &tip[..7]
        )))
        .stdout(predicate::str::contains("feat-1 is 2 behind the pin"));
    assert_eq!(fx.feature_json(&hub, "feat-1")["pins"][0]["sha"], tip);
    assert_eq!(
        fx.git(&ui, &["rev-parse", "feat-1"]),
        branch_tip,
        "never merges"
    );
    let row = status_row(&fx, "ui");
    assert_eq!(row["base_behind"], 2);
    assert_eq!(row["base_moved"], 0);
    fx.hub(&hub_wt(&fx), &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "behind pin {} by 2",
            &tip[..7]
        )))
        .stdout(predicate::str::contains("behind base").not());

    // Unfreezing goes back to origin/develop, which never got d1 and d2.
    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "ui"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ui: unfrozen"));
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());
    let row = status_row(&fx, "ui");
    assert!(row["frozen_sha"].is_null());
    assert_eq!(row["base_behind"], 0);
}

#[test]
fn set_base_refuses_a_pinned_change_until_it_is_unfrozen() {
    let (fx, hub) = world();
    fx.remote_branch("ui-clone", "release");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "ui"])
        .assert()
        .success();
    fx.hub(&hub_wt(&fx), &["feature", "set-base", "ui", "release"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unfreeze first"));
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["base"],
        "develop"
    );

    fx.hub(&hub_wt(&fx), &["feature", "unfreeze", "ui"])
        .assert()
        .success();
    fx.hub(&hub_wt(&fx), &["feature", "set-base", "ui", "release"])
        .assert()
        .success();
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["base"],
        "release"
    );
}

#[test]
fn merged_drops_the_pin_with_the_change() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "ui"])
        .assert()
        .success();
    let pin = fx.feature_json(&hub, "feat-1")["pins"][0]["sha"]
        .as_str()
        .unwrap()
        .to_string();
    fx.hub(&hub, &["feature", "merged", "ui", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "ui: pin at {} dropped",
            &pin[..7]
        )));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["stage"], "merged");
    assert!(f.get("pins").is_none(), "the role is back to absent");
}

fn api_wt(fx: &Fixture) -> PathBuf {
    fx.worktree_dir.join("api-clone").join("feat-1")
}

#[test]
fn add_on_a_frozen_role_starts_from_the_pin_and_keeps_it() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    // The pin is a commit only the local master has.
    let pin = fx.commit_file(&api, "local.txt", "local only");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.commit_file(&api, "later.txt", "master moves past the pin");

    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: removed frozen worktree"));
    assert_eq!(fx.git(&api, &["rev-parse", "feat-1"]), pin);
    assert_eq!(head(&fx, &api_wt(&fx)), pin);
    assert!(!frozen_wt(&fx).exists());
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 2);
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][1]["role"], "api");
    assert_eq!(f["pins"][0]["sha"], pin);
    assert!(f["pins"][0]["worktree"].is_null());
    assert_eq!(
        f["changes"][1]["base_sha"], pin,
        "the change left the base at the pin"
    );
    let row = status_row(&fx, "api");
    assert_eq!(row["frozen_sha"], pin);
    assert_eq!(row["base_behind"], 0);
    assert_eq!(row["base_moved"], 1);
    assert!(
        fx.windows("feat-1")
            .iter()
            .any(|(n, d)| n == "api" && Path::new(d) == api_wt(&fx)),
        "{:?}",
        fx.windows("feat-1")
    );
}

#[test]
fn add_from_a_local_only_pin_is_not_called_merged_after_the_base_is_pushed() {
    let (fx, _hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.commit_file(&api, "local.txt", "local only");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.commit_file(&api, "later.txt", "master moves past the pin");
    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .assert()
        .success();

    // The pin and the commit after it reach origin; feat-1 has no work.
    fx.git(&api, &["push", "-q", "origin", "master"]);
    let row = status_row(&fx, "api");
    assert!(
        row["hint"].is_null(),
        "untouched branch called merged: {row}"
    );
}

#[test]
fn add_from_latest_starts_from_origin_and_drops_the_pin() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.commit_file(&api, "local.txt", "local only");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let latest = fx.remote_sha("api-clone", "master");

    fx.hub(&hub_wt(&fx), &["feature", "add", "api", "--from-latest"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dropped the pin"));
    assert_eq!(fx.git(&api, &["rev-parse", "feat-1"]), latest);
    assert!(!frozen_wt(&fx).exists());
    assert!(fx.feature_json(&hub, "feat-1").get("pins").is_none());
    assert!(status_row(&fx, "api")["frozen_sha"].is_null());
}

#[test]
fn add_adopting_a_branch_rederives_the_pin_from_its_merge_base() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let old = head(&fx, &frozen_wt(&fx));
    let fork = fx.commit_file(&api, "m1.txt", "master moves");
    fx.git(&api, &["checkout", "-q", "-b", "work"]);
    fx.commit_file(&api, "w1.txt", "work");
    fx.git(&api, &["checkout", "-q", "master"]);

    fx.hub(&hub_wt(&fx), &["feature", "add", "api", "--branch", "work"])
        .assert()
        .success();
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["pins"][0]["sha"], fork);
    assert_ne!(f["pins"][0]["sha"], old);
    assert!(f["pins"][0]["worktree"].is_null());
    assert_eq!(f["changes"][1]["base_sha"], fork);
    assert!(!frozen_wt(&fx).exists());
    assert_eq!(status_row(&fx, "api")["base_behind"], 0);
}

/// The frozen role is exactly as `freeze` left it.
fn assert_still_frozen(fx: &Fixture, hub: &Path) {
    let api = fx.project_home.join("api-clone");
    assert!(frozen_wt(fx).exists());
    assert!(!api_wt(fx).exists());
    assert_eq!(fx.git(&api, &["branch", "--list", "feat-1"]), "");
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 2);
    let f = fx.feature_json(hub, "feat-1");
    assert_eq!(f["changes"].as_array().unwrap().len(), 1);
    assert_eq!(f["pins"][0]["worktree"]["name"], "feat-1-frozen");
}

#[test]
fn add_refuses_a_dirty_or_busy_frozen_worktree_and_changes_nothing() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let scratch = frozen_wt(&fx).join("scratch.txt");
    std::fs::write(&scratch, "x\n").unwrap();
    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .assert()
        .code(20);
    assert_still_frozen(&fx, &hub);
    std::fs::remove_file(&scratch).unwrap();

    // Busy is found after open_change: the new branch and worktree roll back.
    let procs = format!("4242\tvite\t{}", frozen_wt(&fx).display());
    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .env("FAKE_LSOF_CWDS", &procs)
        .assert()
        .code(21)
        .stderr(predicate::str::contains("4242"));
    assert_still_frozen(&fx, &hub);
    assert!(
        fx.windows("feat-1")
            .iter()
            .any(|(n, d)| n == "api" && Path::new(d) == frozen_wt(&fx)),
        "window sent back to the reference worktree: {:?}",
        fx.windows("feat-1")
    );
}

#[test]
fn add_refuses_a_frozen_worktree_moved_off_its_pin() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    // A commit on the detached HEAD: removing the worktree would orphan it.
    fx.commit_file(&frozen_wt(&fx), "hand.txt", "committed by hand");
    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("is off its pin"));
    assert_still_frozen(&fx, &hub);
}

#[test]
fn add_leaves_the_window_alone_when_open_change_fails() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let taken = fx.worktree_dir.join("api-clone").join("taken");
    std::fs::create_dir_all(&taken).unwrap();
    let log_before = fx.tmux_log().len();
    // A process runs in the reference worktree; open_change fails first, so
    // the user sees that error and the process keeps running.
    let procs = format!("4242\tvite\t{}", frozen_wt(&fx).display());
    fx.hub(
        &hub_wt(&fx),
        &["feature", "add", "api", "--worktree", "taken"],
    )
    .env("FAKE_LSOF_CWDS", &procs)
    .assert()
    .code(1)
    .stderr(predicate::str::contains("already exists"));
    assert!(
        !fx.tmux_log()[log_before..].contains("respawn-pane"),
        "window re-pointed: {}",
        &fx.tmux_log()[log_before..]
    );
    assert!(
        fx.windows("feat-1")
            .iter()
            .any(|(n, d)| n == "api" && Path::new(d) == frozen_wt(&fx)),
        "{:?}",
        fx.windows("feat-1")
    );
    assert_still_frozen(&fx, &hub);
}

#[test]
fn add_refuses_from_with_an_existing_branch_on_a_frozen_role() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.git(&api, &["branch", "work"]);
    fx.hub(
        &hub_wt(&fx),
        &[
            "feature", "add", "api", "--branch", "work", "--from", "master",
        ],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("--from only starts a new branch"));
    assert!(!fx.worktree_dir.join("api-clone").join("work").exists());
    assert_still_frozen(&fx, &hub);
}

#[test]
fn add_rolls_back_and_keeps_the_frozen_worktree_when_the_record_write_fails() {
    let (fx, hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.block_feature_writes(&hub);
    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .assert()
        .failure();
    fx.allow_feature_writes(&hub);
    assert_still_frozen(&fx, &hub);
}

#[test]
fn add_with_another_base_needs_from_latest_to_leave_the_pin() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "release");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(
        &hub_wt(&fx),
        &["feature", "add", "api", "--base", "release"],
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("--from-latest"));
    assert_still_frozen(&fx, &hub);

    fx.hub(
        &hub_wt(&fx),
        &[
            "feature",
            "add",
            "api",
            "--base",
            "release",
            "--from-latest",
        ],
    )
    .assert()
    .success();
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][1]["base"], "release");
    assert!(f.get("pins").is_none());
}

#[test]
fn add_compares_the_pin_with_the_roles_current_base() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "release");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    // The role's base moves after the freeze; a plain add would merge into
    // release while the pin came from master.
    fx.hub(&hub, &["repo", "set", "api", "--base", "release"])
        .assert()
        .success();
    fx.hub(&hub_wt(&fx), &["feature", "add", "api"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--from-latest"));
    assert_still_frozen(&fx, &hub);
}

#[test]
fn add_refuses_an_adopted_branch_with_no_history_in_common_with_the_base() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.git(&api, &["checkout", "-q", "--orphan", "stray"]);
    fx.commit_file(&api, "stray.txt", "unrelated root");
    fx.git(&api, &["checkout", "-q", "-f", "master"]);

    fx.hub(
        &hub_wt(&fx),
        &["feature", "add", "api", "--branch", "stray"],
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("share no history"));
    assert!(frozen_wt(&fx).exists());
    assert!(!fx.worktree_dir.join("api-clone").join("stray").exists());
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"].as_array().unwrap().len(), 1);
    assert_eq!(f["pins"][0]["worktree"]["name"], "feat-1-frozen");
}

#[test]
fn add_tracking_a_remote_only_branch_rederives_the_pin_from_its_merge_base() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    let old = head(&fx, &frozen_wt(&fx));
    // The local master moves past the pin; origin/work forks from there.
    let fork = fx.commit_file(&api, "m1.txt", "master moves");
    fx.remote_branch("api-clone", "work");
    assert_eq!(fx.git(&api, &["branch", "--list", "work"]), "");

    fx.hub(&hub_wt(&fx), &["feature", "add", "api", "--branch", "work"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tracking origin/work"));
    assert_eq!(
        fx.git(&api, &["rev-parse", "work"]),
        fx.remote_sha("api-clone", "work")
    );
    assert!(!frozen_wt(&fx).exists());
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["pins"][0]["sha"], fork);
    assert_ne!(f["pins"][0]["sha"], old);
    assert!(f["pins"][0]["worktree"].is_null());
    assert_eq!(f["changes"][1]["base_sha"], fork);
}

#[test]
fn add_from_a_ref_starts_there_and_drops_the_pin() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let start = fx.remote_sha("api-clone", "master");
    // The pin is a local commit past `start`.
    fx.commit_file(&api, "local.txt", "local only");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();

    fx.hub(&hub_wt(&fx), &["feature", "add", "api", "--from", &start])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dropped the pin"));
    assert_eq!(fx.git(&api, &["rev-parse", "feat-1"]), start);
    assert!(!frozen_wt(&fx).exists());
    let f = fx.feature_json(&hub, "feat-1");
    assert!(f.get("pins").is_none());
    assert_eq!(f["changes"][1]["base_sha"], start);
}

#[test]
fn add_adopting_a_branch_from_latest_drops_the_pin() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.git(&api, &["branch", "work"]);

    fx.hub(
        &hub_wt(&fx),
        &["feature", "add", "api", "--branch", "work", "--from-latest"],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Dropped the pin"));
    assert!(!frozen_wt(&fx).exists());
    let f = fx.feature_json(&hub, "feat-1");
    assert!(f.get("pins").is_none());
    assert_eq!(f["changes"][1]["branch"], "work");
}
