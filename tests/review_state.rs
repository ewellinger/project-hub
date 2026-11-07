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
    fx.provider_response(
        &format!("{base}/discussions?per_page=100"),
        &json!([{"notes": [{"system": false}]}]),
    );
}

fn viewed(fx: &Fixture, hub: &Path, feature: &str, role: &str) -> assert_cmd::assert::Assert {
    fx.hub(hub, &["feature", "viewed", role, "--feature", feature])
        .assert()
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
        "has_conflicts": false, "head_pipeline": {"status": "running"},
        "sha": "0123456789abcdef0123456789abcdef01234567"
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
fn a_missing_program_still_shows_the_remembered_row() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    refresh(&fx, &hub);

    let assert = fx
        .hub(&hub, &["status", "--feature", "feat-1"])
        .env("PATH", fx.path_without_tmux())
        .assert()
        .success();
    let out = stdout(&assert);

    assert!(
        matches(r"api\s+!412\s+open\s+no\s+running\s+0 open\s+no", &out),
        "{out}"
    );
    let missing = out.lines().filter(|l| l.contains("is not on PATH")).count();
    assert_eq!(missing, 1, "{out}");
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
    assert!(!hub.join(".git/hub/reviews/feat-1.lock").exists());
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

fn status(fx: &Fixture, hub: &Path, args: &[&str]) -> String {
    let mut all = vec!["status", "--feature", "feat-1"];
    all.extend(args);
    stdout(&fx.hub(hub, &all).assert().success())
}

fn matches(pattern: &str, text: &str) -> bool {
    use predicates::prelude::*;
    predicates::str::is_match(pattern).unwrap().eval(text)
}

/// `api` in review with `!412`: open, not approved, pipeline failed, two
/// unresolved threads, no conflicts.
fn api_in_review_with_failed_pipeline(fx: &Fixture, hub: &Path) {
    gitlab_state(
        fx,
        412,
        json!({
            "state": "opened", "draft": false, "merge_status": "can_be_merged",
            "has_conflicts": false, "head_pipeline": {"status": "failed"}
        }),
    );
    let unresolved = json!({"notes": [{"resolvable": true, "resolved": false}]});
    fx.provider_response(
        "projects/g%2Fsub%2Fproj/merge_requests/412/discussions?per_page=100",
        &json!([unresolved, unresolved]),
    );
    review(fx, hub, "feat-1", "api", Some(MR_412));
}

#[test]
fn status_prints_the_review_section_after_the_role_table() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    let out = status(&fx, &hub, &[]);

    assert!(
        matches(
            r"REVIEW\s+REQUEST\s+STATUS\s+APPROVED\s+PIPELINE\s+THREADS\s+CONFLICTS",
            &out
        ),
        "{out}"
    );
    assert!(
        matches(r"api\s+!412\s+open\s+no\s+failed\s+2 open\s+no", &out),
        "{out}"
    );
    assert!(matches(r"review state read \d+s ago", &out), "{out}");
    assert!(
        out.find("ROLE").unwrap() < out.find("REVIEW").unwrap(),
        "{out}"
    );
}

#[test]
fn status_without_a_role_in_review_is_unchanged() {
    let (fx, hub) = world();
    let out = status(&fx, &hub, &[]);

    assert!(!out.contains("REVIEW"), "{out}");
    assert!(!out.contains("review state read"), "{out}");
    assert_eq!(fx.provider_log(), "");
}

#[test]
fn status_json_carries_review_per_row_and_null_elsewhere() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    let json: serde_json::Value = serde_json::from_str(&status(&fx, &hub, &["--json"])).unwrap();
    let row = |role: &str| {
        json["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == role)
            .unwrap()
            .clone()
    };

    assert_eq!(row("api")["review"]["status"], "open");
    assert_eq!(row("api")["review"]["threads"], 2);
    assert_eq!(row("ui")["review"], serde_json::Value::Null);
    assert_eq!(row("hub")["review"], serde_json::Value::Null);
}

#[test]
fn a_merged_request_hints_to_run_feature_merged() {
    let (fx, hub) = world();
    github_state(&fx, 31, json!({"state": "MERGED"}));
    review(&fx, &hub, "feat-1", "ui", Some(PR_31));
    let out = status(&fx, &hub, &[]);

    assert!(
        out.contains("merged on GitHub; run hub feature merged ui"),
        "{out}"
    );
}

/// Push ui's branch, put it in review with `PR_31`, then delete the branch
/// on origin so git hints `gone from origin`.
fn ui_reviewed_and_gone_from_origin(fx: &Fixture, hub: &Path, pr: serde_json::Value) {
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    fx.commit_file(&ui_wt, "work.txt", "work");
    fx.git(&ui_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    github_state(fx, 31, pr);
    review(fx, hub, "feat-1", "ui", Some(PR_31));
    status(fx, hub, &["--json"]);
    fx.git(&ui_wt, &["push", "-q", "origin", "--delete", "feat-1"]);
}

#[test]
fn a_closed_request_replaces_gits_gone_from_origin_hint() {
    let (fx, hub) = world();
    ui_reviewed_and_gone_from_origin(&fx, &hub, json!({"state": "CLOSED"}));
    let out = status(&fx, &hub, &[]);

    assert!(
        out.contains("closed on GitHub without merging; run hub feature dropped ui"),
        "{out}"
    );
    assert!(!out.contains("gone from origin"), "{out}");
}

#[test]
fn an_open_request_leaves_gits_hint_alone() {
    let (fx, hub) = world();
    ui_reviewed_and_gone_from_origin(&fx, &hub, open_pr());
    let out = status(&fx, &hub, &[]);

    assert!(
        out.contains("gone from origin; run hub feature merged ui"),
        "{out}"
    );
}

#[test]
fn a_forge_failure_warns_and_status_still_succeeds() {
    let (fx, hub) = world();
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    let assert = fx
        .hub(&hub, &["status", "--feature", "feat-1"])
        .env("FAKE_PROVIDER_FAIL", "boom")
        .assert()
        .success();
    let out = stdout(&assert);

    assert!(
        out.contains("warning: review state not read for api: "),
        "{out}"
    );
    assert!(
        matches(r"api\s+!412\s+\?\s+\?\s+\?\s+\?\s+\?", &out),
        "{out}"
    );
}

#[test]
fn status_base_never_reads_the_forge() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    fx.hub(&hub, &["status", "--base"]).assert().success();

    assert_eq!(fx.provider_log(), "");
}

#[test]
fn start_review_does_not_read_review_state_during_its_drift_check() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    let hub_wt = fx.worktree_dir.join("acme").join("acme-feat_1");
    assert!(hub_wt.is_dir(), "{}", hub_wt.display());
    fx.provider_response(
        "projects/g%2Fsub%2Fproj",
        &json!({"id": 77, "path_with_namespace": "g/sub/proj"}),
    );

    fx.hub(
        &hub_wt,
        &[
            "feature",
            "start-review",
            "https://gitlab.example.com/g/sub/proj/-/merge_requests/9",
        ],
    )
    .assert()
    .failure();

    let log = fx.provider_log();
    assert!(log.contains("merge_requests/9"), "{log}");
    assert!(!log.contains("approvals"), "{log}");
    assert!(!log.contains("discussions"), "{log}");
}

#[test]
fn list_shows_the_remembered_word_without_reading_the_forge() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    status(&fx, &hub, &[]);
    std::fs::write(&fx.provider_log, "").unwrap();

    let out = stdout(&fx.hub(&hub, &["feature", "list"]).assert().success());

    assert!(out.contains("api:review(ci failed)"), "{out}");
    assert_eq!(fx.provider_log(), "");
}

#[test]
fn list_refresh_shows_the_word_it_just_read() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    let out = stdout(&refresh(&fx, &hub));

    assert!(out.contains("api:review(ci failed)"), "{out}");
    assert!(
        fx.provider_log().contains("merge_requests/412"),
        "{}",
        fx.provider_log()
    );
}

#[test]
fn list_json_carries_the_word() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    review(&fx, &hub, "feat-1", "ui", None);
    let out = stdout(
        &fx.hub(&hub, &["feature", "list", "--refresh", "--json"])
            .assert()
            .success(),
    );
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    let role = |name: &str| {
        json[0]["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == name)
            .unwrap()
            .clone()
    };

    assert_eq!(role("api")["review"], "ci failed");
    assert_eq!(role("ui")["review"], serde_json::Value::Null);
}

#[test]
fn a_finished_feature_shows_no_word() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    assert!(
        stdout(&refresh(&fx, &hub)).contains("api:review(ci failed)"),
        "a word is remembered"
    );
    fx.hub(
        &hub,
        &["feature", "finish", "--force", "--feature", "feat-1"],
    )
    .assert()
    .success();

    let out = stdout(
        &fx.hub(&hub, &["feature", "list", "--all"])
            .assert()
            .success(),
    );

    let row = out
        .lines()
        .find(|l| l.starts_with("feat-1"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(row.contains("finished"), "{out}");
    assert!(row.contains("api:review"), "{out}");
    assert!(!row.contains("api:review("), "{out}");
}

/// Write `viewed` under `role` in the feature's reviews file by hand.
fn seed_mark(fx: &Fixture, hub: &Path, feature: &str, role: &str, viewed: serde_json::Value) {
    let path = hub.join(".git/hub/reviews").join(format!("{feature}.json"));
    let mut file = fx.reviews_json(hub, feature).expect("reviews file");
    file[role]["viewed"] = viewed;
    std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
}

#[test]
fn a_refresh_keeps_the_mark() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    status(&fx, &hub, &[]);
    seed_mark(
        &fx,
        &hub,
        "feat-1",
        "api",
        json!({"at": 1, "head": null, "comments": 0}),
    );

    status(&fx, &hub, &[]);

    let file = fx.reviews_json(&hub, "feat-1").unwrap();
    assert_eq!(file["api"]["viewed"]["comments"], 0, "{file}");
    assert!(
        file["api"]["read_at"].as_u64().unwrap() > 1,
        "fields were refreshed"
    );
}

#[test]
fn viewed_reads_the_forge_and_stores_the_snapshot() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));

    let out = stdout(&viewed(&fx, &hub, "feat-1", "api").success());

    assert!(
        matches(r"api: viewed !412 head [0-9a-f]{7}, 1 comments", &out),
        "{out}"
    );
    let file = fx.reviews_json(&hub, "feat-1").unwrap();
    assert_eq!(file["api"]["viewed"]["comments"], 1);
    assert_eq!(file["api"]["viewed"]["head"], file["api"]["head"]);
    assert!(fx.provider_log().contains("merge_requests/412"));
    assert_eq!(
        fx.feature_json(&hub, "feat-1")["changes"][0]["stage"],
        "review",
        "record untouched"
    );
}

#[test]
fn viewed_refuses_a_role_not_in_review_and_a_failed_read() {
    let (fx, hub) = world();
    viewed(&fx, &hub, "feat-1", "api")
        .failure()
        .stderr(predicates::str::contains(
            "not in review with a merge/pull request URL",
        ));
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    fx.hub(&hub, &["feature", "viewed", "api", "--feature", "feat-1"])
        .env("FAKE_PROVIDER_FAIL", "401 Unauthorized")
        .assert()
        .failure()
        .stderr(predicates::str::contains("review state not read for api"));
    // `refresh` still writes the file (possibly `{}`); only the mark must be absent.
    assert!(
        fx.reviews_json(&hub, "feat-1")
            .is_none_or(|f| f["api"]["viewed"].is_null()),
        "no mark written"
    );
}

#[test]
fn viewed_with_unknown_comments_marks_and_says_so() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    // No discussions answer: the fake fails that one call, the MR itself still reads.
    std::fs::remove_file(
        fx.provider_dir
            .join("projects__g_2Fsub_2Fproj__merge_requests__412__discussions?per_page=100.json"),
    )
    .unwrap();
    review(&fx, &hub, "feat-1", "api", Some(MR_412));

    let out = stdout(&viewed(&fx, &hub, "feat-1", "api").success());

    assert!(out.contains(", ? comments"), "{out}");
    assert_eq!(
        fx.reviews_json(&hub, "feat-1").unwrap()["api"]["viewed"]["comments"],
        serde_json::Value::Null
    );
}

#[test]
fn viewed_succeeds_while_another_command_holds_the_hub_lock() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    let lock = std::fs::File::create(hub.join(".git/hub/hub.lock")).unwrap();
    let mut lock = fd_lock::RwLock::new(lock);
    let _guard = lock.write().unwrap();
    // The lock really is held: a locking command refuses.
    fx.hub(&hub, &["feature", "review", "ui", "--feature", "feat-1"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("another hub command is running"));

    viewed(&fx, &hub, "feat-1", "api").success();

    assert_eq!(
        fx.reviews_json(&hub, "feat-1").unwrap()["api"]["viewed"]["comments"],
        1
    );
}

#[test]
fn a_changed_url_drops_the_mark() {
    let (fx, hub) = world();
    gitlab_state(&fx, 412, open_mr());
    review(&fx, &hub, "feat-1", "api", Some(MR_412));
    viewed(&fx, &hub, "feat-1", "api").success();
    review(
        &fx,
        &hub,
        "feat-1",
        "api",
        Some("https://gitlab.example.com/g/sub/proj/-/merge_requests/413"),
    );

    let out = stdout(&fx.hub(&hub, &["feature", "list"]).assert().success());

    assert!(
        out.contains("api:review") && !out.contains("api:review("),
        "{out}"
    );
}

/// Mark `api` viewed on `open_mr()`, then queue the next answers so the
/// following read sees `next_mr` and `next_discussions`.
fn marked_then_moved(
    fx: &Fixture,
    hub: &Path,
    next_mr: serde_json::Value,
    next_discussions: serde_json::Value,
) {
    gitlab_state(fx, 412, open_mr());
    review(fx, hub, "feat-1", "api", Some(MR_412));
    viewed(fx, hub, "feat-1", "api").success();
    let base = "projects/g%2Fsub%2Fproj/merge_requests/412";
    fx.provider_response(base, &next_mr);
    fx.provider_response(
        &format!("{base}/discussions?per_page=100"),
        &next_discussions,
    );
}

fn one_note() -> serde_json::Value {
    json!([{"notes": [{"system": false}]}])
}

#[test]
fn a_push_after_the_mark_shows_pushed_in_list_and_status() {
    let (fx, hub) = world();
    let mut moved = open_mr();
    moved["sha"] = json!("ffffffffffffffffffffffffffffffffffffffff");
    marked_then_moved(&fx, &hub, moved, one_note());

    let out = status(&fx, &hub, &[]);
    assert!(out.contains("pushed since viewed"), "{out}");
    let out = stdout(&fx.hub(&hub, &["feature", "list"]).assert().success());
    assert!(out.contains("api:review(pushed)"), "{out}");
}

#[test]
fn replies_after_the_mark_show_a_count() {
    let (fx, hub) = world();
    let three = json!([{"notes": [{"system": false}, {"system": false}, {"system": true}]},
        {"notes": [{"system": false}]}]);
    marked_then_moved(&fx, &hub, open_mr(), three);

    let out = status(&fx, &hub, &[]);
    assert!(out.contains("2 replies since viewed"), "{out}");
    let out = stdout(&fx.hub(&hub, &["feature", "list"]).assert().success());
    assert!(out.contains("api:review(2 replies)"), "{out}");
}

#[test]
fn merged_outranks_pushed_in_word_and_hint() {
    let (fx, hub) = world();
    let mut moved = open_mr();
    moved["sha"] = json!("ffffffffffffffffffffffffffffffffffffffff");
    moved["state"] = json!("merged");
    marked_then_moved(&fx, &hub, moved, one_note());

    let out = status(&fx, &hub, &[]);
    assert!(
        out.contains("merged on GitLab; run hub feature merged api"),
        "{out}"
    );
    assert!(!out.contains("pushed"), "{out}");
    let out = stdout(&fx.hub(&hub, &["feature", "list"]).assert().success());
    assert!(out.contains("api:review(merged)"), "{out}");
}

#[test]
fn status_json_carries_head_comments_and_viewed() {
    let (fx, hub) = world();
    marked_then_moved(&fx, &hub, open_mr(), one_note());
    github_state(&fx, 31, open_pr());
    review(&fx, &hub, "feat-1", "ui", Some(PR_31));
    let json: serde_json::Value = serde_json::from_str(&status(&fx, &hub, &["--json"])).unwrap();
    let row = |role: &str| {
        json["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["role"] == role)
            .unwrap()
            .clone()
    };

    assert_eq!(row("api")["review"]["viewed"]["comments"], 1);
    assert_eq!(row("api")["review"]["comments"], 1);
    assert_eq!(row("api")["review"]["head"].as_str().unwrap().len(), 40);
    assert_eq!(
        row("ui")["review"].get("viewed"),
        Some(&serde_json::Value::Null),
        "the key is present and null until marked"
    );
}

#[test]
fn a_mark_set_while_status_reads_the_forge_survives_its_write() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    status(&fx, &hub, &[]); // the file exists, with no mark
    std::thread::scope(|scope| {
        scope.spawn(|| {
            fx.hub(&hub, &["status", "--feature", "feat-1"])
                .env("FAKE_PROVIDER_SLEEP", "3")
                .assert()
                .success();
        });
        // Status has snapshotted the file and is waiting on the forge.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        seed_mark(
            &fx,
            &hub,
            "feat-1",
            "api",
            json!({"at": 1, "head": null, "comments": 0}),
        );
    });

    let file = fx.reviews_json(&hub, "feat-1").unwrap();
    assert_eq!(file["api"]["viewed"]["comments"], 0, "{file}");
}

/// Hold the reviews lock while `status` runs, call `write`, then release.
/// Status must still be waiting at that point: an unlocked write would have
/// gone through already, and would overwrite `write` in a slower interleaving.
fn write_while_status_waits_for_the_lock(
    fx: &Fixture,
    hub: &Path,
    failing_forge: bool,
    write: impl FnOnce(),
) {
    let lock = std::fs::File::create(hub.join(".git/hub/reviews/feat-1.lock")).unwrap();
    let mut lock = fd_lock::RwLock::new(lock);
    let guard = lock.write().unwrap();
    let mut cmd = fx.hub(hub, &["status", "--feature", "feat-1"]);
    if failing_forge {
        cmd.env("FAKE_PROVIDER_FAIL", "boom");
    }
    let early = std::thread::scope(|scope| {
        let status = scope.spawn(move || {
            cmd.assert().success();
        });
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let early = status.is_finished();
        write();
        drop(guard);
        early
    });
    assert!(!early, "status wrote without taking the lock");
}

#[test]
fn a_mark_written_just_before_status_saves_survives() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    status(&fx, &hub, &[]);
    write_while_status_waits_for_the_lock(&fx, &hub, false, || {
        seed_mark(
            &fx,
            &hub,
            "feat-1",
            "api",
            json!({"at": 1, "head": null, "comments": 0}),
        );
    });

    let file = fx.reviews_json(&hub, "feat-1").unwrap();
    assert_eq!(file["api"]["viewed"]["comments"], 0, "{file}");
}

#[test]
fn a_mark_on_a_role_this_status_could_not_read_survives() {
    let (fx, hub) = world();
    api_in_review_with_failed_pipeline(&fx, &hub);
    status(&fx, &hub, &[]);
    let path = hub.join(".git/hub/reviews/feat-1.json");
    let saved = std::fs::read_to_string(&path).unwrap();
    // Status starts with nothing remembered for api, and its forge read fails.
    std::fs::write(&path, "{}").unwrap();
    write_while_status_waits_for_the_lock(&fx, &hub, true, || {
        std::fs::write(&path, &saved).unwrap();
        seed_mark(
            &fx,
            &hub,
            "feat-1",
            "api",
            json!({"at": 1, "head": null, "comments": 0}),
        );
    });

    let file = fx.reviews_json(&hub, "feat-1").unwrap();
    assert_eq!(file["api"]["viewed"]["comments"], 0, "{file}");
}
