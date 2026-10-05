mod common;

use std::path::Path;

use common::Fixture;
use hub::error::HubError;
use hub::hub::Hub;
use predicates::prelude::*;

fn world() -> (Fixture, std::path::PathBuf) {
    let fx = Fixture::new();
    let hub = fx.project_home.join("acme");
    std::fs::create_dir_all(&hub).unwrap();
    fx.hub(
        &hub,
        &["init", "--checkout-template", "acme-{feature_snake}"],
    )
    .assert()
    .success();
    fx.make_repo("api-clone", "master");
    fx.make_repo("bff-clone", "develop");
    fx.make_repo("ui-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "bff", "bff-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    (fx, hub)
}

#[test]
fn starts_a_feature_across_repos() {
    let (fx, hub) = world();
    fx.hub(
        &hub,
        &[
            "feature",
            "start",
            "port-e2e",
            "--repo",
            "ui",
            "--repo",
            "api:feature/acme/port",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "Attach with: tmux attach -t acme-port_e2e",
    ));

    let hub_wt = fx.worktree_dir.join("acme").join("acme-port_e2e");
    assert_eq!(
        fx.git(&hub_wt, &["symbolic-ref", "--short", "HEAD"]),
        "port-e2e"
    );
    assert_eq!(fx.git(&hub, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");

    let f = fx.feature_json(&hub, "port-e2e");
    assert_eq!(f["checkout"], "acme-port_e2e");
    assert_eq!(f["status"], "open");
    let changes = f["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["role"], "api", "manifest order, not flag order");
    assert_eq!(changes[0]["branch"], "feature/acme/port");
    assert_eq!(changes[0]["worktree"]["name"], "feature-acme-port");
    assert_eq!(changes[1]["role"], "ui");
    assert_eq!(changes[1]["branch"], "port-e2e");
    assert!(
        fx.worktree_dir
            .join("api-clone")
            .join("feature-acme-port")
            .is_dir()
    );
    assert!(fx.worktree_dir.join("ui-clone").join("port-e2e").is_dir());
    assert!(!fx.worktree_dir.join("bff-clone").exists());

    assert_eq!(
        fx.window_names("acme-port_e2e"),
        vec!["hub", "ai", "api", "ui"]
    );
    let windows = fx.windows("acme-port_e2e");
    assert_eq!(windows[0].1, hub_wt.display().to_string());
    assert_eq!(windows[1].1, hub_wt.display().to_string());
    assert!(windows[2].1.ends_with("api-clone/feature-acme-port"));
    assert!(!fx.tmux_log().contains("attach"));

    let ws = hub_wt.join("acme.code-workspace");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ws).unwrap()).unwrap();
    let folders = json["folders"].as_array().unwrap();
    assert_eq!(folders[0]["name"], "acme");
    assert_eq!(folders[0]["path"], ".");
    assert_eq!(folders[1]["name"], "api");
    assert_eq!(folders[1]["path"], "../../api-clone/feature-acme-port");
    assert_eq!(
        fx.git(&hub_wt, &["status", "--porcelain"]),
        "",
        "workspace file is ignored"
    );
}

#[test]
fn workspace_is_locally_ignored_for_hubs_created_before_the_ignore_rule() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    std::fs::write(hub.join(".gitignore"), ".DS_Store\n.tmp/\n").unwrap();
    fx.git(&hub, &["add", ".gitignore"]);
    fx.git(&hub, &["commit", "-q", "-m", "remove workspace ignore"]);

    fx.hub(&hub, &["feature", "start", "feat-1"])
        .assert()
        .success();

    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    assert!(hub_wt.join("acme.code-workspace").exists());
    assert_eq!(fx.git(&hub_wt, &["status", "--porcelain"]), "");
    assert_eq!(
        std::fs::read_to_string(hub.join(".gitignore")).unwrap(),
        ".DS_Store\n.tmp/\n",
        "the compatibility rule stays local"
    );
}

#[test]
fn zero_repos_and_checkout_override() {
    let (fx, hub) = world();
    fx.hub(
        &hub,
        &["feature", "start", "docs-only", "--checkout", "custom"],
    )
    .assert()
    .success();
    let f = fx.feature_json(&hub, "docs-only");
    assert_eq!(f["checkout"], "custom");
    assert_eq!(f["changes"].as_array().unwrap().len(), 0);
    assert!(fx.worktree_dir.join("acme").join("custom").is_dir());
    assert_eq!(fx.window_names("custom"), vec!["hub", "ai"]);
}

#[test]
fn adopts_existing_hub_branch_and_existing_session() {
    let (fx, hub) = world();
    fx.git(&hub, &["branch", "feat-1"]);
    let hub_wt = fx.worktree_dir.join("acme").join("acme-feat_1");
    fx.seed_session("acme-feat_1", &[("hub", "/old")]);
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "bff"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Adopting existing hub branch"));
    assert!(hub_wt.is_dir());
    assert_eq!(fx.window_names("acme-feat_1"), vec!["hub", "ai", "bff"]);
    assert_eq!(
        fx.windows("acme-feat_1")[0].1,
        "/old",
        "existing windows are left alone"
    );
}

#[test]
fn refuses_bad_input_before_creating_anything() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "bad.name"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("feature name"));
    fx.hub(&hub, &["feature", "start", "feat-1", "--checkout", "a:b"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("checkout name"));
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "nope"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("unknown role"));
    fx.hub(
        &hub,
        &["feature", "start", "feat-1", "--repo", "ui", "--repo", "ui"],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("more than once"));
    assert!(!hub.join(".git/hub/features/feat-1.json").exists());
    assert_eq!(fx.git(&hub, &["branch", "--list", "feat-1"]), "");
    assert!(!fx.worktree_dir.join("acme").exists());

    fx.hub(&hub, &["feature", "start", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "start", "feat-1"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn preflight_matches_starts_checks() {
    let (fx, hub) = world();
    let h = Hub::locate(&hub, fx.env()).unwrap();

    assert_eq!(
        hub::ops::start::preflight(&h, "feat-1", None).unwrap(),
        "acme-feat_1",
        "templated checkout name"
    );

    let err = hub::ops::start::preflight(&h, "bad.name", None).unwrap_err();
    assert!(matches!(err, HubError::Usage(m) if m.contains("feature name")));

    fx.hub(&hub, &["feature", "start", "feat-1"])
        .assert()
        .success();
    let err = hub::ops::start::preflight(&h, "feat-1", None).unwrap_err();
    assert!(matches!(err, HubError::Precondition(m) if m.contains("already exists")));
}

#[test]
fn rolls_back_everything_when_a_later_repo_fails() {
    let (fx, hub) = world();
    let ui = fx.project_home.join("ui-clone");
    fx.git(
        &ui,
        &["remote", "set-url", "origin", "git@example.com:changed.git"],
    );
    fx.hub(
        &hub,
        &[
            "feature", "start", "feat-1", "--repo", "api", "--repo", "ui",
        ],
    )
    .assert()
    .code(1)
    .stderr(predicate::str::contains("origin"));
    assert!(!hub.join(".git/hub/features/feat-1.json").exists());
    assert_eq!(fx.git(&hub, &["branch", "--list", "feat-1"]), "");
    assert!(!fx.worktree_dir.join("acme").join("acme-feat_1").exists());
    let api = fx.project_home.join("api-clone");
    assert_eq!(fx.git(&api, &["branch", "--list", "feat-1"]), "");
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 1);
    assert!(!fx.has_session("acme-feat_1"));
    assert_eq!(
        fx.git(&hub, &["log", "-1", "--format=%s"]),
        "hub: repo add ui"
    );
}

#[test]
fn rolls_back_everything_when_the_record_write_fails() {
    let (fx, hub) = world();
    fx.block_feature_writes(&hub);
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Permission denied"));
    assert!(!hub.join(".git/hub/features/feat-1.json").exists());
    assert_eq!(
        fx.git(&hub, &["status", "--porcelain"]),
        "",
        "main is never touched"
    );
    assert_eq!(fx.git(&hub, &["branch", "--list", "feat-1"]), "");
    assert!(!fx.worktree_dir.join("acme").join("acme-feat_1").exists());
    let api = fx.project_home.join("api-clone");
    assert_eq!(fx.git(&api, &["branch", "--list", "feat-1"]), "");
    assert!(!fx.worktree_dir.join("api-clone").join("feat-1").exists());
    assert!(
        !fx.has_session("acme-feat_1"),
        "tmux runs only after the record is written"
    );
    fx.allow_feature_writes(&hub);
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
}

#[test]
fn warns_and_keeps_state_when_tmux_fails_after_the_write() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .env("FAKE_TMUX_FAIL", "new-session")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("warning:")
                .and(predicate::str::contains("hub tmux --feature feat-1")),
        );
    assert_eq!(fx.feature_json(&hub, "feat-1")["status"], "open");
    assert!(
        fx.worktree_dir.join("api-clone").join("feat-1").is_dir(),
        "not rolled back"
    );
    assert!(
        fx.worktree_dir
            .join("acme")
            .join("acme-feat_1")
            .join("acme.code-workspace")
            .exists()
    );
    assert!(!fx.has_session("acme-feat_1"));
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .assert()
        .success();
    assert_eq!(fx.window_names("acme-feat_1"), vec!["hub", "ai", "api"]);
}

#[test]
fn copies_ignored_local_config_into_new_worktrees() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.exclude_locally(&api, &[".env", ".vscode/"]);
    std::fs::create_dir_all(api.join(".vscode")).unwrap();
    std::fs::write(api.join(".vscode/settings.json"), "{\"a\":1}").unwrap();
    std::fs::write(api.join(".env"), "K=v\n").unwrap();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    assert_eq!(
        std::fs::read_to_string(wt.join(".vscode/settings.json")).unwrap(),
        "{\"a\":1}"
    );
    assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "K=v\n");
    assert!(Path::new(&wt).join("README.md").exists());
    assert_eq!(
        fx.git(&wt, &["status", "--porcelain"]),
        "",
        "copies are ignored, so the worktree is clean"
    );
}

#[test]
fn adopts_the_hub_branch_from_origin() {
    let (fx, hub) = world();
    let bare = fx.remotes.join("hub.git");
    fx.git(
        &fx.remotes,
        &["init", "-q", "--bare", "-b", "main", "hub.git"],
    );
    fx.git(&hub, &["remote", "add", "origin", bare.to_str().unwrap()]);
    fx.git(&hub, &["push", "-q", "-u", "origin", "main"]);
    // A teammate pushed the hub branch: create it, push it, drop the local copy.
    fx.git(&hub, &["branch", "feat-1"]);
    let seed = fx.worktree_dir.join("seed");
    fx.git(
        &hub,
        &["worktree", "add", "-q", seed.to_str().unwrap(), "feat-1"],
    );
    fx.commit_file(&seed, "plan.md", "teammate plan");
    fx.git(&seed, &["push", "-q", "origin", "feat-1"]);
    fx.git(&hub, &["worktree", "remove", seed.to_str().unwrap()]);
    fx.git(&hub, &["branch", "-D", "feat-1"]);
    fx.git(&hub, &["fetch", "-q", "--prune", "origin"]);

    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adopting hub branch feat-1 from origin",
        ));
    let hub_wt = fx.worktree_dir.join("acme").join("acme-feat_1");
    assert_eq!(
        fx.git(&hub_wt, &["symbolic-ref", "--short", "HEAD"]),
        "feat-1"
    );
    assert_eq!(
        fx.git(&hub_wt, &["rev-parse", "--abbrev-ref", "feat-1@{upstream}"]),
        "origin/feat-1"
    );
    assert!(
        hub_wt.join("plan.md").exists(),
        "the teammate's work is checked out"
    );
}

#[test]
fn warns_when_the_hub_fetch_fails() {
    let (fx, hub) = world();
    let missing = fx.remotes.join("gone.git");
    fx.git(
        &hub,
        &["remote", "add", "origin", missing.to_str().unwrap()],
    );
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning: fetch failed in"))
        .stdout(predicate::str::contains("using the last fetched refs"));
    let hub_wt = fx.worktree_dir.join("acme").join("acme-feat_1");
    assert_eq!(
        fx.git(&hub_wt, &["symbolic-ref", "--short", "HEAD"]),
        "feat-1"
    );
    assert_eq!(
        fx.git(&hub, &["rev-parse", "feat-1"]),
        fx.git(&hub, &["rev-parse", "main"]),
        "a fresh branch from main"
    );
}

#[test]
fn rolls_back_an_adopted_hub_branch_when_a_later_step_fails() {
    let (fx, hub) = world();
    let bare = fx.remotes.join("hub.git");
    fx.git(
        &fx.remotes,
        &["init", "-q", "--bare", "-b", "main", "hub.git"],
    );
    fx.git(&hub, &["remote", "add", "origin", bare.to_str().unwrap()]);
    fx.git(&hub, &["push", "-q", "-u", "origin", "main"]);
    // A teammate pushed the hub branch: create it, push it, drop the local copy.
    fx.git(&hub, &["branch", "feat-1"]);
    let seed = fx.worktree_dir.join("seed");
    fx.git(
        &hub,
        &["worktree", "add", "-q", seed.to_str().unwrap(), "feat-1"],
    );
    fx.commit_file(&seed, "plan.md", "teammate plan");
    fx.git(&seed, &["push", "-q", "origin", "feat-1"]);
    fx.git(&hub, &["worktree", "remove", seed.to_str().unwrap()]);
    fx.git(&hub, &["branch", "-D", "feat-1"]);
    fx.git(&hub, &["fetch", "-q", "--prune", "origin"]);

    fx.block_feature_writes(&hub);
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Permission denied"));
    assert_eq!(
        fx.git(&hub, &["branch", "--list", "feat-1"]),
        "",
        "the adopted tracking branch was rolled back"
    );
    assert!(!fx.worktree_dir.join("acme").join("acme-feat_1").exists());
    fx.allow_feature_writes(&hub);
}

#[test]
fn a_new_session_starts_hub_in_its_hub_window_once() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "master");
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success();
    let checkout = fx.feature_json(&hub, "feat-1")["checkout"]
        .as_str()
        .unwrap()
        .to_string();
    let expected = format!("send-keys -t ={checkout}:hub hub Enter");
    assert_eq!(
        fx.tmux_log().matches(&expected).count(),
        1,
        "{}",
        fx.tmux_log()
    );
    // Converging an existing session leaves its hub window alone.
    fx.hub(&hub, &["tmux", "--feature", "feat-1"])
        .assert()
        .success();
    assert_eq!(fx.tmux_log().matches("send-keys").count(), 1);
}

#[test]
fn a_failed_hub_launch_still_builds_every_window() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .env("FAKE_TMUX_FAIL", "send-keys")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "run hub in the session's hub window",
        ));
    assert_eq!(fx.window_names("acme-feat_1"), vec!["hub", "ai", "api"]);
}
