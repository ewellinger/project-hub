mod common;

use std::path::{Path, PathBuf};

use common::Fixture;
use serde_json::json;

const GITLAB_REMOTE: &str = "https://gitlab.example.com/g/sub/proj.git";
const GITHUB_REMOTE: &str = "git@github.example.com:Owner/Repo.git";
const MR_412: &str = "https://gitlab.example.com/g/sub/proj/-/merge_requests/412";
const PR_31: &str = "https://github.example.com/Owner/Repo/pull/31";

/// Hub `acme` with `api` on a GitLab-shaped remote, `ui` on a GitHub-shaped
/// one, and `bff` on a plain filesystem remote; `feat-1` on api and ui.
fn world() -> (Fixture, PathBuf) {
    let fx = Fixture::new();
    let hub = fx.project_home.join("acme");
    std::fs::create_dir_all(&hub).unwrap();
    fx.hub(
        &hub,
        &["init", "--checkout-template", "acme-{feature_snake}"],
    )
    .assert()
    .success();
    fx.make_provider_repo("api-clone", "master", GITLAB_REMOTE);
    fx.make_provider_repo("ui-clone", "develop", GITHUB_REMOTE);
    fx.make_repo("bff-clone", "develop");
    fx.add_repo(&hub, "api", "api-clone");
    fx.add_repo(&hub, "ui", "ui-clone");
    fx.add_repo(&hub, "bff", "bff-clone");
    start(&fx, &hub, "feat-1", &["api", "ui"]);
    (fx, hub)
}

fn start(fx: &Fixture, hub: &Path, name: &str, roles: &[&str]) {
    let mut args = vec!["feature", "start", name];
    for role in roles {
        args.extend(["--repo", role]);
    }
    fx.hub(hub, &args).assert().success();
}

fn review(fx: &Fixture, hub: &Path, feature: &str, role: &str, url: Option<&str>) {
    let mut args = vec!["feature", "review", role, "--feature", feature];
    args.extend(url);
    fx.hub(hub, &args).assert().success();
}

fn refresh(fx: &Fixture, hub: &Path) -> assert_cmd::assert::Assert {
    fx.hub(hub, &["feature", "list", "--refresh"])
        .assert()
        .success()
}

fn stdout(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

/// Store the three GitLab answers for MR `!number` of `g/sub/proj`.
fn gitlab_state(fx: &Fixture, number: u64, mr: serde_json::Value) {
    let base = format!("projects/g%2Fsub%2Fproj/merge_requests/{number}");
    fx.provider_response(&base, &mr);
    fx.provider_response(&format!("{base}/approvals"), &json!({"approved": false}));
    fx.provider_response(&format!("{base}/discussions?per_page=100"), &json!([]));
}

/// Store the GraphQL answer for PR `#number`.
fn github_state(fx: &Fixture, number: u64, pr: serde_json::Value) {
    fx.provider_response(
        &format!("graphql/{number}"),
        &json!({"data": {"repository": {"pullRequest": pr}}}),
    );
}

fn open_mr() -> serde_json::Value {
    json!({
        "state": "opened", "draft": false, "merge_status": "can_be_merged",
        "has_conflicts": false, "head_pipeline": {"status": "running"}
    })
}

fn open_pr() -> serde_json::Value {
    json!({
        "state": "OPEN", "isDraft": false, "mergeable": "MERGEABLE",
        "reviewDecision": "REVIEW_REQUIRED",
        "reviewThreads": {"totalCount": 0, "nodes": []},
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}]}
    })
}

#[test]
fn refresh_stores_one_state_per_role_in_review() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    github_state(&fx, 31, open_pr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    review(&fx, &hub, "feat-1", "ui", Some(PR_31));
    refresh(&fx, &hub);

    let file = fx.reviews_json(&hub, "feat-1").expect("reviews file");
    let keys: Vec<&String> = file.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["api", "ui"]);
    assert_eq!(file["api"]["status"], "open");
    assert_eq!(file["ui"]["pipeline"], "passed");
    assert_eq!(file["api"]["url"], MR_412);
    assert_eq!(file["ui"]["url"], PR_31);
    for role in ["api", "ui"] {
        assert!(file[role]["read_at"].as_u64().unwrap() > 0);
    }
}

#[test]
fn a_role_without_a_url_or_still_working_is_not_a_target() {
    let (fx, hub) = world();
    review(&fx, &hub, "feat-1", "api", None);
    let out = stdout(&refresh(&fx, &hub));

    assert!(!out.contains("warning"), "{out}");
    assert_eq!(fx.reviews_json(&hub, "feat-1"), None);
    assert_eq!(fx.provider_log(), "");
}

#[test]
fn an_unparseable_recorded_url_is_skipped_without_a_warning() {
    let (fx, hub) = world();
    review(
        &fx,
        &hub,
        "feat-1",
        "api",
        Some("https://gitlab.example.com/not/a/request"),
    );
    let out = stdout(&refresh(&fx, &hub));

    assert!(!out.contains("warning"), "{out}");
    assert_eq!(fx.reviews_json(&hub, "feat-1"), None);
    assert_eq!(fx.provider_log(), "");
}

#[test]
fn a_failed_read_keeps_the_remembered_state_and_warns() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    refresh(&fx, &hub);
    let before = fx.reviews_json(&hub, "feat-1").expect("reviews file");

    let assert = fx
        .hub(&hub, &["feature", "list", "--refresh"])
        .env("FAKE_PROVIDER_FAIL", "boom")
        .assert()
        .success();
    let out = stdout(&assert);

    assert_eq!(fx.reviews_json(&hub, "feat-1"), Some(before));
    let warnings: Vec<&str> = out.lines().filter(|l| l.starts_with("warning:")).collect();
    assert_eq!(warnings.len(), 1, "{out}");
    assert!(
        warnings[0].starts_with("warning: review state not read for api: "),
        "{out}"
    );
    assert!(warnings[0].contains("boom"), "{out}");
    assert_eq!(
        out.lines().last(),
        Some(warnings[0]),
        "warning after the table"
    );
}

#[test]
fn a_missing_program_warns_once_for_several_roles() {
    let (fx, hub) = world();
    start(&fx, &hub, "feat-2", &["api"]);
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    review(
        &fx,
        &hub,
        "feat-2",
        "api",
        Some("https://gitlab.example.com/g/sub/proj/-/merge_requests/413"),
    );

    let assert = fx
        .hub(&hub, &["feature", "list", "--refresh"])
        .env("PATH", fx.path_without_tmux())
        .assert()
        .success();
    let out = stdout(&assert);

    assert_eq!(out.matches("glab is not on PATH").count(), 1, "{out}");
}

#[test]
fn a_changed_url_makes_the_remembered_state_absent() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    refresh(&fx, &hub);
    assert!(
        fx.reviews_json(&hub, "feat-1")
            .unwrap()
            .get("api")
            .is_some()
    );

    review(
        &fx,
        &hub,
        "feat-1",
        "api",
        Some("https://gitlab.example.com/g/sub/proj/-/merge_requests/413"),
    );
    refresh(&fx, &hub);

    let file = fx.reviews_json(&hub, "feat-1").expect("reviews file");
    assert_eq!(file.get("api"), None, "{file}");
}

#[test]
fn a_corrupt_reviews_file_is_treated_as_absent_and_overwritten() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    let dir = hub.join(".git/hub/reviews");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("feat-1.json"), "{not json").unwrap();

    refresh(&fx, &hub);

    let file = fx.reviews_json(&hub, "feat-1").expect("reviews file");
    assert_eq!(file["api"]["status"], "open");
}

#[test]
fn feature_remove_deletes_the_reviews_file() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    refresh(&fx, &hub);
    assert!(fx.reviews_json(&hub, "feat-1").is_some());

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
        .success();

    assert!(!hub.join(".git/hub/reviews/feat-1.json").exists());
}

#[test]
fn refresh_reads_every_open_feature_with_a_target() {
    let (fx, hub) = world();
    start(&fx, &hub, "feat-2", &["api"]);
    start(&fx, &hub, "feat-3", &["api"]);
    gitlab_state(&fx, 412, open_mr());
    github_state(&fx, 31, open_pr());
    review(&fx, &hub, "feat-1", "ui", Some(PR_31));
    review(&fx, &hub, "feat-2", "api", Some(MR_412));

    refresh(&fx, &hub);

    assert!(fx.reviews_json(&hub, "feat-1").unwrap().get("ui").is_some());
    assert!(
        fx.reviews_json(&hub, "feat-2")
            .unwrap()
            .get("api")
            .is_some()
    );
    assert_eq!(fx.reviews_json(&hub, "feat-3"), None);
}

#[test]
fn refresh_with_json_sends_warnings_to_stderr() {
    let (fx, hub) = world();
    review(&fx, &hub, "feat-1", "api", Some(MR_412));

    let assert = fx
        .hub(&hub, &["feature", "list", "--refresh", "--json"])
        .env("FAKE_PROVIDER_FAIL", "boom")
        .assert()
        .success();
    let output = assert.get_output();

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(parsed.is_array());
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(
        stderr.contains("warning: review state not read for api: "),
        "{stderr}"
    );
    assert!(stderr.contains("boom"), "{stderr}");
}
