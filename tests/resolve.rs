mod common;

use common::Fixture;
use hub::error::HubError;
use hub::feature::{Feature, Owner, Stage};
use hub::hub::Hub;
use hub::ops::resolve::{AddOptions, Created, open_change};

struct World {
    fx: Fixture,
    hub: Hub,
    feature: Feature,
}

fn world() -> World {
    let fx = Fixture::new();
    let hub_dir = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub_dir, "api", "api-clone");
    fx.hub(
        &hub_dir,
        &[
            "repo",
            "add",
            "ui",
            "ui-clone",
            "--branch-template",
            "pc_{feature}-ew",
        ],
    )
    .assert()
    .success();
    let hub = Hub::locate(&hub_dir, fx.env()).unwrap();
    let feature = Feature::new("feat-1", "acme-feat_1");
    World { fx, hub, feature }
}

fn opts(role: &str) -> AddOptions<'_> {
    AddOptions {
        role,
        branch: None,
        from: None,
        worktree: None,
        base: None,
    }
}

#[test]
fn creates_branch_from_base_and_a_worktree() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    w.fx.exclude_locally(&clone, &[".env", ".vscode/"]);
    std::fs::create_dir_all(clone.join(".vscode")).unwrap();
    std::fs::write(clone.join(".vscode/settings.json"), "{}").unwrap();
    std::fs::write(clone.join(".env"), "X=1").unwrap();
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert_eq!(opened.change.role, "api");
    assert_eq!(opened.change.clone, "api-clone");
    assert_eq!(opened.change.branch, "feat-1");
    assert_eq!(opened.change.stage, Stage::Working);
    assert!(!opened.change.origin_seen);
    assert_eq!(
        opened.change.base_sha.as_deref(),
        Some(w.fx.git(&clone, &["rev-parse", "origin/master"]).as_str())
    );
    assert_eq!(opened.change.base.as_deref(), Some("master"));
    let wt = opened.change.worktree.as_ref().unwrap();
    assert_eq!(wt.name, "feat-1");
    assert_eq!(wt.owner, Owner::Hub);
    let expected = w.fx.worktree_dir.join("api-clone").join("feat-1");
    assert_eq!(opened.path, expected);
    assert!(expected.join(".vscode/settings.json").exists());
    assert!(expected.join(".env").exists());
    assert_eq!(
        w.fx.git(&expected, &["status", "--porcelain"]),
        "",
        "copied files are ignored, so the worktree starts clean"
    );
    assert_eq!(
        w.fx.git(&expected, &["symbolic-ref", "--short", "HEAD"]),
        "feat-1"
    );
    assert_eq!(
        w.fx.git(&expected, &["rev-parse", "HEAD"]),
        w.fx.git(&clone, &["rev-parse", "origin/master"])
    );
    assert_eq!(
        w.fx.git(
            &clone,
            &["for-each-ref", "--format=%(upstream)", "refs/heads/feat-1"]
        ),
        "",
        "no upstream on a new branch"
    );
    assert_eq!(
        created.branches,
        vec![(clone.clone(), "feat-1".to_string())]
    );
    assert_eq!(created.worktrees, vec![(clone, expected)]);
    assert!(opened.lines.iter().any(|l| l.contains("Created branch")));
}

#[test]
fn skips_local_config_the_clone_does_not_ignore() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    std::fs::write(clone.join(".env"), "X=1").unwrap();
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert!(!opened.path.join(".env").exists());
    assert!(
        opened.lines.iter().any(|l| l.contains("skipped .env")),
        "{:?}",
        opened.lines
    );
    assert_eq!(
        w.fx.git(&opened.path, &["status", "--porcelain"]),
        "",
        "new worktree starts clean"
    );
}

#[test]
fn refuses_worktree_names_that_are_paths_before_touching_anything() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    let outside = w.fx.root.join("escape");
    for bad in [outside.to_str().unwrap(), "../escape", ".", "..", "a/b"] {
        let mut created = Created::new();
        let o = AddOptions {
            role: "api",
            branch: None,
            from: None,
            worktree: Some(bad),
            base: None,
        };
        let err = open_change(&w.hub, &w.feature, &o, &mut created).unwrap_err();
        assert!(
            matches!(err, HubError::Usage(m) if m.contains("worktree name")),
            "{bad}"
        );
        assert!(
            created.branches.is_empty() && created.worktrees.is_empty(),
            "{bad}"
        );
    }
    assert_eq!(
        w.fx.git(&clone, &["branch", "--list", "feat-1"]),
        "",
        "rejected before any branch was made"
    );
    assert!(!outside.exists());
    assert!(!w.fx.worktree_dir.join("api-clone").exists());
    assert_eq!(w.fx.git(&clone, &["worktree", "list"]).lines().count(), 1);
}

#[test]
fn uses_branch_template_and_explicit_overrides() {
    let w = world();
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("ui"), &mut created).unwrap();
    assert_eq!(opened.change.branch, "pc_feat-1-ew");
    assert_eq!(opened.change.worktree.unwrap().name, "pc_feat-1-ew");

    let clone = w.fx.project_home.join("api-clone");
    let base = w.fx.git(&clone, &["rev-parse", "origin/master"]);
    w.fx.git(&clone, &["branch", "start-here", &base]);
    let o = AddOptions {
        role: "api",
        branch: Some("feature/ACME-1/x"),
        from: Some("start-here"),
        worktree: Some("custom"),
        base: None,
    };
    let opened = open_change(&w.hub, &w.feature, &o, &mut created).unwrap();
    assert_eq!(opened.change.branch, "feature/ACME-1/x");
    assert_eq!(opened.change.worktree.unwrap().name, "custom");
    assert!(opened.path.ends_with("api-clone/custom"));
}

#[test]
fn flattens_slashes_in_created_worktree_names() {
    let w = world();
    let mut created = Created::new();
    let o = AddOptions {
        role: "api",
        branch: Some("feature/acme/x"),
        from: None,
        worktree: None,
        base: None,
    };
    let opened = open_change(&w.hub, &w.feature, &o, &mut created).unwrap();
    assert_eq!(opened.change.worktree.unwrap().name, "feature-acme-x");
}

#[test]
fn tracks_an_existing_origin_branch() {
    let w = world();
    w.fx.remote_branch("api-clone", "feat-1");
    let clone = w.fx.project_home.join("api-clone");
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert!(opened.change.origin_seen);
    assert_eq!(
        w.fx.git(&clone, &["rev-parse", "feat-1"]),
        w.fx.git(&clone, &["rev-parse", "origin/feat-1"])
    );
    assert_eq!(
        w.fx.git(
            &clone,
            &["for-each-ref", "--format=%(upstream)", "refs/heads/feat-1"]
        ),
        "refs/remotes/origin/feat-1"
    );
    assert_eq!(created.branches.len(), 1);
    assert!(
        opened
            .lines
            .iter()
            .any(|l| l.contains("tracking origin/feat-1"))
    );
}

#[test]
fn reuses_a_local_branch_and_fast_forwards_when_behind() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    w.fx.remote_branch("api-clone", "feat-1");
    let remote_tip = w.fx.git(&clone, &["rev-parse", "origin/feat-1"]);
    w.fx.git(&clone, &["branch", "feat-1", "origin/master"]);
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert_eq!(w.fx.git(&clone, &["rev-parse", "feat-1"]), remote_tip);
    assert!(created.branches.is_empty());
    assert!(opened.lines.iter().any(|l| l.contains("Fast-forwarded")));

    let mut created = Created::new();
    let o = AddOptions {
        role: "ui",
        branch: Some("local-only"),
        from: None,
        worktree: None,
        base: None,
    };
    let ui = w.fx.project_home.join("ui-clone");
    w.fx.git(&ui, &["branch", "local-only"]);
    let opened = open_change(&w.hub, &w.feature, &o, &mut created).unwrap();
    assert!(created.branches.is_empty());
    assert!(
        opened
            .lines
            .iter()
            .any(|l| l.contains("Reusing branch local-only"))
    );
}

#[test]
fn refuses_a_diverged_branch() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    w.fx.remote_branch("api-clone", "feat-1");
    w.fx.git(&clone, &["checkout", "-q", "-b", "feat-1", "origin/master"]);
    w.fx.commit_file(&clone, "local.txt", "local work");
    w.fx.git(&clone, &["checkout", "-q", "master"]);
    let mut created = Created::new();
    let err = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("diverged")));
    assert!(created.branches.is_empty() && created.worktrees.is_empty());
}

#[test]
fn adopts_a_branch_checked_out_in_the_main_clone() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    w.fx.git(&clone, &["checkout", "-q", "-b", "feat-1"]);
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert_eq!(opened.change.worktree, None);
    assert_eq!(opened.path, clone);
    assert!(created.worktrees.is_empty());
    assert!(opened.lines.iter().any(|l| l.contains("main clone")));
}

#[test]
fn adopts_a_review_worktree_under_worktree_dir() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    w.fx.remote_branch("api-clone", "feat-1");
    let review = w.fx.worktree_dir.join("api-clone").join("api-review_368");
    std::fs::create_dir_all(review.parent().unwrap()).unwrap();
    w.fx.git(
        &clone,
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
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    let wt = opened.change.worktree.unwrap();
    assert_eq!(wt.name, "api-review_368");
    assert_eq!(wt.owner, Owner::Adopted);
    assert_eq!(opened.path, review);
    assert!(created.worktrees.is_empty());

    let o = AddOptions {
        role: "api",
        branch: None,
        from: None,
        worktree: Some("other"),
        base: None,
    };
    let err = open_change(&w.hub, &w.feature, &o, &mut created).unwrap_err();
    assert!(
        matches!(err, HubError::Usage(m) if m.contains("--worktree") && m.contains("api-review_368"))
    );
}

#[test]
fn refuses_a_branch_checked_out_outside_worktree_dir() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    let elsewhere = w.fx.root.join("elsewhere");
    w.fx.git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat-1",
            elsewhere.to_str().unwrap(),
            "origin/master",
        ],
    );
    let mut created = Created::new();
    let err = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap_err();
    assert!(
        matches!(err, HubError::Precondition(m) if m.contains("elsewhere") && m.contains("<worktree_dir>"))
    );
}

#[test]
fn refuses_path_collisions_and_bad_preconditions() {
    let mut w = world();
    let taken = w.fx.worktree_dir.join("api-clone").join("feat-1");
    std::fs::create_dir_all(&taken).unwrap();
    let mut created = Created::new();
    let err = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("already exists")));
    std::fs::remove_dir_all(&taken).unwrap();

    assert!(matches!(
        open_change(&w.hub, &w.feature, &opts("nope"), &mut created).unwrap_err(),
        HubError::Usage(_)
    ));

    let o = AddOptions {
        role: "api",
        branch: Some("bad..name"),
        from: None,
        worktree: None,
        base: None,
    };
    assert!(matches!(
        open_change(&w.hub, &w.feature, &o, &mut created).unwrap_err(),
        HubError::Usage(_)
    ));

    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    w.feature.push_change(opened.change).unwrap();
    let err = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("already has an open change")));

    let ui = w.fx.project_home.join("ui-clone");
    w.fx.git(
        &ui,
        &["remote", "set-url", "origin", "git@example.com:changed.git"],
    );
    let err = open_change(&w.hub, &w.feature, &opts("ui"), &mut created).unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("origin")));
}

#[test]
fn rollback_removes_only_what_was_created() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    let ui = w.fx.project_home.join("ui-clone");
    w.fx.git(&ui, &["branch", "pre-existing"]);
    let mut created = Created::new();
    open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    let o = AddOptions {
        role: "ui",
        branch: Some("pre-existing"),
        from: None,
        worktree: None,
        base: None,
    };
    let ui_opened = open_change(&w.hub, &w.feature, &o, &mut created).unwrap();
    assert!(ui_opened.path.exists());
    let warnings = created.rollback();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!w.fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert!(!ui_opened.path.exists());
    assert_eq!(w.fx.git(&clone, &["branch", "--list", "feat-1"]), "");
    assert_eq!(
        w.fx.git(&ui, &["branch", "--list", "pre-existing"]).trim(),
        "pre-existing"
    );
}

#[test]
fn follow_up_branch_after_a_merged_change() {
    let mut w = world();
    let clone = w.fx.project_home.join("api-clone");
    let mut created = Created::new();
    let mut first = open_change(&w.hub, &w.feature, &opts("api"), &mut created)
        .unwrap()
        .change;
    first.stage = Stage::Merged;
    w.feature.push_change(first).unwrap();
    w.fx.git(&clone, &["branch", "feat-1-2"]);
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert_eq!(opened.change.branch, "feat-1-3");
    assert_eq!(opened.change.worktree.unwrap().name, "feat-1-3");
}

#[test]
fn explicit_base_is_recorded_separately_from_the_start_point() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    w.fx.remote_branch("api-clone", "feature/canonical");
    let fork = w.fx.git(&clone, &["rev-parse", "origin/master"]);
    w.fx.git(&clone, &["branch", "start-here", &fork]);
    let o = AddOptions {
        role: "api",
        branch: None,
        from: Some("start-here"),
        worktree: None,
        base: Some("feature/canonical"),
    };
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &o, &mut created).unwrap();
    assert_eq!(opened.change.base.as_deref(), Some("feature/canonical"));
    assert_eq!(
        w.fx.git(&clone, &["rev-parse", "feat-1"]),
        fork,
        "created from --from, not from the base"
    );
    assert_eq!(
        opened.change.base_sha.as_deref(),
        Some(fork.as_str()),
        "merge base of the branch and origin/feature/canonical, not the base tip"
    );
    assert_ne!(
        fork,
        w.fx.git(&clone, &["rev-parse", "origin/feature/canonical"])
    );
}

#[test]
fn reused_branch_records_its_fork_point_as_base_sha() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    let fork = w.fx.git(&clone, &["rev-parse", "origin/master"]);
    w.fx.git(&clone, &["branch", "feat-1", &fork]);
    let tip = w.fx.commit_file(&clone, "later.txt", "later");
    w.fx.git(&clone, &["push", "-q", "origin", "master"]);
    let mut created = Created::new();
    let opened = open_change(&w.hub, &w.feature, &opts("api"), &mut created).unwrap();
    assert_eq!(opened.change.base_sha.as_deref(), Some(fork.as_str()));
    assert_ne!(fork, tip);
    assert!(created.branches.is_empty(), "reused, not created");
}

#[test]
fn refuses_a_bad_or_absent_base_before_touching_anything() {
    let w = world();
    let clone = w.fx.project_home.join("api-clone");
    for bad in ["origin/master", "bad..name"] {
        let o = AddOptions {
            role: "api",
            branch: None,
            from: None,
            worktree: None,
            base: Some(bad),
        };
        let mut created = Created::new();
        let err = open_change(&w.hub, &w.feature, &o, &mut created).unwrap_err();
        assert!(
            matches!(err, HubError::Usage(ref m) if m.contains("--base")),
            "{bad}: {err}"
        );
        assert!(created.branches.is_empty(), "{bad}");
    }
    let o = AddOptions {
        role: "api",
        branch: None,
        from: None,
        worktree: None,
        base: Some("not-there"),
    };
    let mut created = Created::new();
    let err = open_change(&w.hub, &w.feature, &o, &mut created).unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("origin/not-there")));
    assert!(created.branches.is_empty());
    assert_eq!(w.fx.git(&clone, &["branch", "--list", "feat-1"]), "");
}
