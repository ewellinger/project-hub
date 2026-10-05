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
fn set_base_records_the_base_and_fork_point_without_touching_the_branch() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    let fork = fx.git(&api, &["rev-parse", "origin/master"]);
    // origin/feature/canonical is one commit past the fork point.
    fx.remote_branch("api-clone", "feature/canonical");
    let canonical_tip = fx.git(&api, &["rev-parse", "origin/feature/canonical"]);
    let branch_tip = fx.commit_file(&wt, "work.txt", "work");
    std::fs::write(wt.join("scratch.txt"), "uncommitted\n").unwrap();

    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(
        &hub_wt,
        &["feature", "set-base", "api", "feature/canonical"],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "api: base feature/canonical (was master)",
    ))
    .stdout(predicate::str::contains(&fork));

    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["base"], "feature/canonical");
    assert_eq!(
        f["changes"][0]["base_sha"], fork,
        "fork point, not the base tip"
    );
    assert_ne!(fork, canonical_tip);
    assert_eq!(
        f["changes"][1]["base"], "develop",
        "the other role is untouched"
    );
    assert_eq!(fx.git(&wt, &["rev-parse", "HEAD"]), branch_tip);
    assert_eq!(fx.git(&wt, &["symbolic-ref", "--short", "HEAD"]), "feat-1");
    assert!(wt.join("scratch.txt").exists(), "working tree untouched");
    assert_eq!(
        fx.git(
            &api,
            &["for-each-ref", "--format=%(upstream)", "refs/heads/feat-1"]
        ),
        "",
        "git's upstream is not where the hub keeps this"
    );

    // Same request again: nothing to do, record unchanged.
    let before = fx.feature_json(&hub, "feat-1");
    fx.hub(
        &hub,
        &[
            "feature",
            "set-base",
            "api",
            "feature/canonical",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("already"));
    assert_eq!(fx.feature_json(&hub, "feat-1"), before);
}

#[test]
fn set_base_rejects_bad_input_without_changing_state() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    let original_sha = fx.git(&api, &["rev-parse", "origin/master"]);
    let cases: [(&[&str], &str); 4] = [
        (
            &[
                "feature",
                "set-base",
                "nope",
                "master",
                "--feature",
                "feat-1",
            ],
            "unknown role",
        ),
        (
            &[
                "feature",
                "set-base",
                "api",
                "origin/master",
                "--feature",
                "feat-1",
            ],
            "without origin/",
        ),
        (
            &[
                "feature",
                "set-base",
                "api",
                "bad..name",
                "--feature",
                "feat-1",
            ],
            "not a valid branch name",
        ),
        (
            &[
                "feature",
                "set-base",
                "api",
                "not-there",
                "--feature",
                "feat-1",
            ],
            "origin/not-there",
        ),
    ];
    for (args, message) in cases {
        fx.hub(&hub, args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains(message));
    }

    // Unrelated history on origin.
    let empty_tree = fx.git(&api, &["hash-object", "-t", "tree", "/dev/null"]);
    let island = fx.git(&api, &["commit-tree", &empty_tree, "-m", "island"]);
    fx.git(
        &api,
        &[
            "push",
            "-q",
            "origin",
            &format!("{island}:refs/heads/island"),
        ],
    );
    fx.hub(
        &hub,
        &[
            "feature",
            "set-base",
            "api",
            "island",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("no history"));

    // A role without an open change.
    fx.hub(&hub, &["feature", "merged", "ui", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(
        &hub,
        &[
            "feature",
            "set-base",
            "ui",
            "develop",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("no open change"));

    // A finished feature.
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(
        &hub,
        &[
            "feature",
            "set-base",
            "api",
            "master",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("finished"));

    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"][0]["base"], "master");
    assert_eq!(f["changes"][0]["base_sha"], original_sha);
}
