mod common;

use std::path::PathBuf;

use common::Fixture;
use hub::error::HubError;
use hub::feature::{Change, Feature, Stage};
use hub::hub::Hub;
use hub::ops::suggest::{Suggestion, addable_roles, default_branch, suggestions};

struct World {
    fx: Fixture,
    hub: Hub,
    clone: PathBuf,
}

/// One role `api` on base `master` with: a local-only branch, a branch on
/// both sides, an origin-only branch containing the feature name, a branch
/// checked out under $GIT_WORKTREE_DIR/api-clone/, and one checked out
/// somewhere the hub refuses to adopt.
fn world() -> World {
    let fx = Fixture::new();
    let hub_dir = fx.init_hub("acme");
    let clone = fx.make_repo("api-clone", "master");
    fx.add_repo(&hub_dir, "api", "api-clone");
    fx.git(&clone, &["branch", "local-only"]);
    fx.git(&clone, &["branch", "both"]);
    fx.git(&clone, &["push", "-q", "origin", "both"]);
    fx.remote_branch("api-clone", "feat-1-origin");
    let adoptable = fx.worktree_dir.join("api-clone").join("adoptable");
    std::fs::create_dir_all(adoptable.parent().unwrap()).unwrap();
    fx.git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "adoptable",
            adoptable.to_str().unwrap(),
        ],
    );
    let elsewhere = fx.root.join("elsewhere");
    fx.git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "elsewhere",
            elsewhere.to_str().unwrap(),
        ],
    );
    let hub = Hub::locate(&hub_dir, fx.env()).unwrap();
    World { fx, hub, clone }
}

fn names(list: &[Suggestion]) -> Vec<&str> {
    list.iter().map(|s| s.branch.as_str()).collect()
}

#[test]
fn lists_known_branches_in_spec_order_without_the_base_or_foreign_worktrees() {
    let w = world();
    let feature = Feature::new("feat-1", "feat-1");
    let list = suggestions(&w.hub, &feature, "api").unwrap();
    assert_eq!(
        names(&list),
        vec!["feat-1-origin", "adoptable", "both", "local-only"],
        "feature-name match first, then by name; no master, no elsewhere"
    );
    let by_name = |n: &str| list.iter().find(|s| s.branch == n).unwrap();
    assert!(by_name("local-only").local && !by_name("local-only").origin);
    assert!(!by_name("feat-1-origin").local && by_name("feat-1-origin").origin);
    assert!(by_name("both").local && by_name("both").origin);
    assert_eq!(
        by_name("adoptable").worktree.as_deref(),
        Some(
            w.fx.worktree_dir
                .join("api-clone")
                .join("adoptable")
                .as_path()
        )
    );
    assert_eq!(by_name("both").worktree, None);
}

#[test]
fn default_branch_leads_when_it_exists_and_is_absent_otherwise() {
    let w = world();
    let feature = Feature::new("feat-1", "feat-1");
    assert_eq!(default_branch(&w.hub, &feature, "api").unwrap(), "feat-1");
    let before = suggestions(&w.hub, &feature, "api").unwrap();
    assert!(!names(&before).contains(&"feat-1"));
    w.fx.git(&w.clone, &["branch", "feat-1"]);
    let after = suggestions(&w.hub, &feature, "api").unwrap();
    assert_eq!(&names(&after)[..2], &["feat-1", "feat-1-origin"]);
}

#[test]
fn main_clone_checkout_counts_as_adoptable() {
    let w = world();
    w.fx.git(&w.clone, &["checkout", "-q", "local-only"]);
    let feature = Feature::new("feat-1", "feat-1");
    let list = suggestions(&w.hub, &feature, "api").unwrap();
    let s = list.iter().find(|s| s.branch == "local-only").unwrap();
    assert_eq!(s.worktree.as_deref(), Some(w.clone.as_path()));
}

#[test]
fn follow_up_default_after_a_merged_change() {
    let w = world();
    w.fx.git(&w.clone, &["branch", "feat-1"]);
    let mut feature = Feature::new("feat-1", "feat-1");
    feature.changes.push(Change {
        role: "api".into(),
        clone: "api-clone".into(),
        branch: "feat-1".into(),
        worktree: None,
        stage: Stage::Merged,
        review_url: None,
        merged_at: Some("2026-09-09T00:00:00Z".into()),
        origin_seen: false,
        base: None,
        base_sha: None,
    });
    assert_eq!(default_branch(&w.hub, &feature, "api").unwrap(), "feat-1-2");
    let list = suggestions(&w.hub, &feature, "api").unwrap();
    assert_eq!(
        names(&list)[0],
        "feat-1",
        "the default feat-1-2 does not exist; feat-1 leads because it contains the feature name"
    );
}

#[test]
fn addable_roles_filters_out_roles_with_an_open_change() {
    let w = world();
    let mut feature = Feature::new("feat-1", "feat-1");
    let roles = addable_roles(&w.hub, &feature).unwrap();
    assert_eq!(
        roles.iter().map(|r| r.role.as_str()).collect::<Vec<_>>(),
        vec!["api"]
    );

    feature.changes.push(Change {
        role: "api".into(),
        clone: "api-clone".into(),
        branch: "feat-1".into(),
        worktree: None,
        stage: Stage::Working,
        review_url: None,
        merged_at: None,
        origin_seen: false,
        base: None,
        base_sha: None,
    });
    let err = addable_roles(&w.hub, &feature).unwrap_err();
    assert!(matches!(
        err,
        HubError::Precondition(m) if m == "every role already has an open change in feat-1"
    ));
}

#[test]
fn missing_clone_is_a_precondition_error() {
    let w = world();
    std::fs::remove_dir_all(&w.clone).unwrap();
    let feature = Feature::new("feat-1", "feat-1");
    let err = suggestions(&w.hub, &feature, "api").unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("missing")));
}
