mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::Fixture;
use predicates::prelude::*;

const ENV_SCRIPT: &str = "env | grep '^HUB_' | sort > setup-env.txt\npwd >> setup-env.txt\n";

/// Hub `acme` with `api` (setup `scripts/api.sh`) and `ui` (no setup).
fn world() -> (Fixture, PathBuf) {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_repo("api-clone", "main");
    fx.make_repo("ui-clone", "main");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.hub(&hub, &["repo", "set", "api", "--setup", "scripts/api.sh"])
        .assert()
        .success();
    (fx, hub)
}

/// Write `<hub>/<rel>` with a `#!/bin/sh` line and `mode`, committed on main.
fn script(fx: &Fixture, hub: &Path, rel: &str, body: &str, mode: u32) {
    let path = hub.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    fx.git(hub, &["add", rel]);
    fx.git(hub, &["commit", "-q", "-m", "setup script"]);
}

fn env_file(fx: &Fixture, clone: &str, worktree: &str) -> PathBuf {
    fx.worktree_dir
        .join(clone)
        .join(worktree)
        .join("setup-env.txt")
}

#[test]
fn start_runs_the_setup_script_in_the_new_worktree() {
    let (fx, hub) = world();
    script(&fx, &hub, "scripts/api.sh", ENV_SCRIPT, 0o755);
    let out = fx
        .hub(
            &hub,
            &[
                "feature", "start", "feat-1", "--repo", "api", "--repo", "ui",
            ],
        )
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Running setup for api: scripts/api.sh",
        ))
        .stdout(predicate::str::contains("api: setup ok ("))
        .stdout(predicate::str::contains("Running setup for ui").not())
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(
        text.find("Running setup for api").unwrap() < text.find("api: setup ok").unwrap(),
        "the Running line comes first:\n{text}"
    );
    let wt = fx.worktree_dir.join("api-clone").join("feat-1");
    let seen = std::fs::read_to_string(env_file(&fx, "api-clone", "feat-1")).unwrap();
    let lines: Vec<&str> = seen.lines().collect();
    assert_eq!(
        lines,
        vec![
            format!("HUB_BASE={}", fx.project_home.join("api-clone").display()),
            "HUB_BRANCH=feat-1".to_string(),
            "HUB_FEATURE=feat-1".to_string(),
            "HUB_ROLE=api".to_string(),
            format!("HUB_ROOT={}", hub.display()),
            format!("HUB_WORKTREE={}", wt.display()),
            wt.display().to_string(),
        ]
    );
    assert!(hub.join(".git/hub/setup/feat-1/api.log").is_file());
}

#[test]
fn a_failing_script_is_reported_and_the_command_still_succeeds() {
    let (fx, hub) = world();
    script(
        &fx,
        &hub,
        "scripts/api.sh",
        "echo boom >&2\nexit 3\n",
        0o755,
    );
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: setup failed (exit 3)"))
        .stdout(predicate::str::contains("; log "))
        .stdout(predicate::str::contains("\n    boom\n"));
    let f = fx.feature_json(&hub, "feat-1");
    assert_eq!(f["changes"].as_array().unwrap().len(), 1);
}

#[test]
fn a_missing_or_non_executable_script_could_not_start() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: setup could not start:"));
    script(&fx, &hub, "scripts/api.sh", "exit 0\n", 0o644);
    fx.hub(&hub, &["feature", "start", "feat-2", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: setup could not start:"))
        .stdout(predicate::str::contains("not executable"));
}

#[test]
fn no_setup_skips_the_script() {
    let (fx, hub) = world();
    script(&fx, &hub, "scripts/api.sh", ENV_SCRIPT, 0o755);
    fx.hub(
        &hub,
        &["feature", "start", "feat-1", "--repo", "api", "--no-setup"],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Running setup").not());
    assert!(!env_file(&fx, "api-clone", "feat-1").exists());
}

#[test]
fn add_runs_setup_but_adopted_and_frozen_worktrees_do_not() {
    let (fx, hub) = world();
    script(&fx, &hub, "scripts/api.sh", ENV_SCRIPT, 0o755);
    fx.hub(&hub, &["feature", "start", "feat-1"])
        .assert()
        .success();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(&hub_wt, &["feature", "add", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("api: setup ok ("));
    assert!(env_file(&fx, "api-clone", "feat-1").is_file());

    let clone = fx.project_home.join("api-clone");
    let adopted = fx.worktree_dir.join("api-clone").join("feat-2");
    fx.git(
        &clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat-2",
            adopted.to_str().unwrap(),
        ],
    );
    fx.hub(&hub, &["feature", "start", "feat-2", "--repo", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Adopting worktree"))
        .stdout(predicate::str::contains("Running setup").not());
    assert!(!env_file(&fx, "api-clone", "feat-2").exists());

    fx.hub(&hub, &["feature", "start", "feat-3", "--freeze", "api"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Running setup").not());
}

const GITLAB_REMOTE: &str = "https://gitlab.example.com/g/sub/proj.git";

#[test]
fn start_review_runs_setup() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.make_provider_repo("api-clone", "master", GITLAB_REMOTE);
    fx.add_repo(&hub, "api", "api-clone");
    fx.hub(&hub, &["repo", "set", "api", "--setup", "scripts/api.sh"])
        .assert()
        .success();
    script(&fx, &hub, "scripts/api.sh", ENV_SCRIPT, 0o755);
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    fx.provider_response(
        "projects/g%2Fsub%2Fproj",
        &serde_json::json!({"id": 77, "path_with_namespace": "g/sub/proj"}),
    );
    fx.provider_response(
        "projects/g%2Fsub%2Fproj/merge_requests/486",
        &serde_json::json!({
            "iid": 486, "state": "opened", "draft": false,
            "web_url": "https://gitlab.example.com/g/sub/proj/-/merge_requests/486",
            "source_project_id": 77, "target_project_id": 77,
            "source_branch": "feature/x", "target_branch": "master", "sha": sha
        }),
    );
    fx.hub(
        &hub,
        &[
            "feature",
            "start-review",
            "https://gitlab.example.com/g/sub/proj/-/merge_requests/486",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("api: setup ok ("));
    let seen = std::fs::read_to_string(env_file(&fx, "api-clone", "feature-x")).unwrap();
    assert!(seen.contains("HUB_FEATURE=api-review-486\n"), "{seen}");
    assert!(seen.contains("HUB_BRANCH=feature/x\n"), "{seen}");
}
