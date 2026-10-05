mod common;

use std::path::{Path, PathBuf};

use common::Fixture;
use predicates::prelude::*;

/// Hub `acme` with roles `api` (base `master`) and `ui` (base `develop`),
/// and feature `feat-1` open in both.
fn world() -> (Fixture, PathBuf) {
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

/// One commit pushed to `origin/<branch>` of `clone` from a separate
/// checkout, so the fixture's own checkouts are behind.
fn push_from_elsewhere(fx: &Fixture, clone: &str, branch: &str, file: &str) -> String {
    let bare = fx.remotes.join(format!("{clone}.git"));
    let dir = fx.root.join(format!("elsewhere-{clone}-{file}"));
    fx.git(
        &fx.root,
        &["clone", "-q", bare.to_str().unwrap(), dir.to_str().unwrap()],
    );
    fx.git(&dir, &["checkout", "-q", branch]);
    let sha = fx.commit_file(&dir, file, "teammate work");
    fx.git(&dir, &["push", "-q", "origin", branch]);
    sha
}

fn head(fx: &Fixture, dir: &Path) -> String {
    fx.git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn pull_from_the_main_hub_fast_forwards_base_checkouts_and_skips_dirty_ones() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let ui = fx.project_home.join("ui-clone");
    let api_tip = push_from_elsewhere(&fx, "api-clone", "master", "api.txt");
    push_from_elsewhere(&fx, "ui-clone", "develop", "ui.txt");
    let ui_before = head(&fx, &ui);
    std::fs::write(ui.join("scratch.txt"), "wip\n").unwrap();
    let hub_head = head(&fx, &hub);

    fx.hub(&hub, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "api: master fast-forwarded 1 commit\n",
        ))
        .stdout(predicate::str::contains(
            "ui: skipped, develop is 1 behind but the checkout is dirty",
        ));

    assert_eq!(head(&fx, &api), api_tip);
    assert_eq!(fx.git(&api, &["symbolic-ref", "--short", "HEAD"]), "master");
    assert_eq!(head(&fx, &ui), ui_before, "the dirty checkout did not move");
    assert!(ui.join("scratch.txt").is_file());
    assert_eq!(
        head(&fx, &hub),
        hub_head,
        "the hub's own clone is not pulled"
    );
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");

    // Second run: nothing left to do for api, ui still refused.
    fx.hub(&hub, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: master is up to date"))
        .stdout(predicate::str::contains("ui: skipped, develop is 1 behind"));
    std::fs::remove_file(ui.join("scratch.txt")).unwrap();
    fx.hub(&hub, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "ui: develop fast-forwarded 1 commit",
        ));
}

#[test]
fn pull_from_a_feature_worktree_fast_forwards_its_open_changes() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    fx.git(&api_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    let api_tip = push_from_elsewhere(&fx, "api-clone", "feat-1", "shared.txt");
    let ui_before = head(&fx, &ui_wt);
    let api_main = head(&fx, &fx.project_home.join("api-clone"));

    fx.hub(&hub_wt, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "api: feat-1 fast-forwarded 1 commit\n",
        ))
        .stdout(predicate::str::contains(
            "ui: skipped, feat-1 has no origin/feat-1",
        ));
    assert_eq!(head(&fx, &api_wt), api_tip);
    assert!(api_wt.join("shared.txt").is_file());
    assert_eq!(head(&fx, &ui_wt), ui_before);
    assert_eq!(
        head(&fx, &fx.project_home.join("api-clone")),
        api_main,
        "the base checkout is not touched by a feature pull"
    );
    let record = fx.feature_json(&hub, "feat-1");
    assert_eq!(record["changes"][0]["stage"], "working", "record untouched");

    // The same feature by name from the main hub, and the bases from the worktree.
    fx.hub(&hub, &["pull", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: feat-1 is up to date"));
    fx.hub(&hub_wt, &["pull", "--base"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: master is up to date"))
        .stdout(predicate::str::contains("ui: develop is up to date"));

    // A merged role has no checkout to pull; a finished feature refuses.
    fx.hub(&hub_wt, &["feature", "merged", "api"])
        .assert()
        .success();
    fx.hub(&hub_wt, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ui: skipped"))
        .stdout(predicate::str::contains("api:").not());
    fx.hub(&hub_wt, &["feature", "finish"]).assert().success();
    fx.hub(&hub, &["pull", "--feature", "feat-1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("finished"));
    fx.hub(&hub, &["pull", "--feature", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nope"));
}

#[test]
fn pull_with_no_repos_and_diverged_bases_reports_and_changes_nothing() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.hub(&hub, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no repos registered"));

    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    let api = fx.project_home.join("api-clone");
    let local = fx.commit_file(&api, "local.txt", "local work");
    push_from_elsewhere(&fx, "api-clone", "master", "remote.txt");
    fx.hub(&hub, &["pull"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "api: skipped, master has diverged from origin/master (+1/-1)",
        ));
    assert_eq!(head(&fx, &api), local);
}
