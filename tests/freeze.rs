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
