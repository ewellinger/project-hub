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
fn freeze_refuses_open_changes_repeats_unknown_roles_and_occupied_paths() {
    let (fx, _hub) = world();
    let wt = hub_wt(&fx);
    fx.hub(&wt, &["feature", "freeze", "ui"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has an open change"));
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
        .failure();
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
fn add_refuses_a_frozen_role_until_it_is_unfrozen() {
    let (fx, _hub) = world();
    fx.hub(&hub_wt(&fx), &["feature", "freeze", "api"])
        .assert()
        .success();
    fx.hub(
        &hub_wt(&fx),
        &["feature", "add", "api", "--branch", "feat-1"],
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("is frozen"))
    .stderr(predicate::str::contains("hub feature unfreeze api"));
    assert!(frozen_wt(&fx).exists());
}
