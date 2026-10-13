mod common;

use common::Fixture;
use predicates::prelude::*;

/// `feat-1` on api and ui, with work committed on both member branches.
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
    for clone in ["api-clone", "ui-clone"] {
        fx.commit_file(&fx.worktree_dir.join(clone).join("feat-1"), "w.txt", "work");
    }
    (fx, hub)
}

fn branch_exists(fx: &Fixture, dir: &std::path::Path, branch: &str) -> bool {
    !fx.git(dir, &["branch", "--list", branch]).is_empty()
}

fn record_path(hub: &std::path::Path) -> std::path::PathBuf {
    hub.join(".git/hub/features/feat-1.json")
}

#[test]
fn remove_deletes_merged_branches_and_the_record_so_the_name_is_free() {
    let (fx, hub) = world();
    for role in ["api", "ui"] {
        fx.hub(&hub, &["feature", "merged", role, "--feature", "feat-1"])
            .assert()
            .success();
    }
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "remove", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed feature 'feat-1'"));
    for clone in ["api-clone", "ui-clone"] {
        assert!(!branch_exists(&fx, &fx.project_home.join(clone), "feat-1"));
    }
    assert!(!branch_exists(&fx, &hub, "feat-1"), "hub branch deleted");
    assert!(!record_path(&hub).exists());

    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Reusing").not())
        .stdout(predicate::str::contains("Adopting").not());
}

#[test]
fn remove_refuses_an_open_feature() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "remove", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("hub feature finish"));
    assert!(record_path(&hub).exists());
}

#[test]
fn remove_refuses_unmerged_and_pushed_branches_and_changes_nothing() {
    let (fx, hub) = world();
    // api merged but still on origin; ui never marked merged.
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.git(&api_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "remove", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("api: feat-1 is still on origin"))
        .stderr(predicate::str::contains("ui: feat-1 is not merged"));
    for clone in ["api-clone", "ui-clone"] {
        assert!(branch_exists(&fx, &fx.project_home.join(clone), "feat-1"));
    }
    assert!(branch_exists(&fx, &hub, "feat-1"));
    assert!(record_path(&hub).exists());
}

#[test]
fn remove_refuses_a_hub_branch_with_commits_and_skips_gone_branches() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.commit_file(&hub_wt, "notes.md", "notes");
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    // Branches deleted by hand count as already removed.
    for clone in ["api-clone", "ui-clone"] {
        fx.git(&fx.project_home.join(clone), &["branch", "-D", "feat-1"]);
    }
    fx.hub(&hub, &["feature", "remove", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "hub: feat-1 has commits not on main",
        ));
    assert!(record_path(&hub).exists());

    fx.git(&hub, &["branch", "-D", "feat-1"]);
    fx.hub(&hub, &["feature", "remove", "feat-1"])
        .assert()
        .success();
    assert!(!record_path(&hub).exists());
}
