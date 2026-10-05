mod common;

use std::path::PathBuf;

use common::Fixture;
use predicates::prelude::*;

const GITLAB_REMOTE: &str = "https://gitlab.example.com/g/sub/proj.git";
const GITHUB_REMOTE: &str = "git@github.example.com:Owner/Repo.git";

/// Hub `acme` with `api` on a GitLab-shaped remote, `ui` on a GitHub-shaped
/// one, and `bff` on a plain filesystem remote.
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
    (fx, hub)
}

/// Store an open, same-project GitLab MR `!number` from `source` into `target` at `sha`.
fn gitlab_mr(fx: &Fixture, number: u64, source: &str, target: &str, sha: &str) {
    fx.provider_response(
        "projects/g%2Fsub%2Fproj",
        &serde_json::json!({"id": 77, "path_with_namespace": "g/sub/proj"}),
    );
    fx.provider_response(
        &format!("projects/g%2Fsub%2Fproj/merge_requests/{number}"),
        &serde_json::json!({
            "iid": number, "state": "opened", "draft": false,
            "web_url": format!("https://gitlab.example.com/g/sub/proj/-/merge_requests/{number}"),
            "source_project_id": 77, "target_project_id": 77,
            "source_branch": source, "target_branch": target, "sha": sha
        }),
    );
}

/// Store an open, same-repository GitHub PR `#number` from `source` into `target` at `sha`.
///
/// `path` is the `owner/repo` spelling the test drives the CLI with: `hub` asks the
/// provider for the path exactly as the URL wrote it, and the fake provider's lookup
/// is case-sensitive wherever the filesystem is.
fn github_pr(fx: &Fixture, path: &str, number: u64, source: &str, target: &str, sha: &str) {
    fx.provider_response(
        &format!("repos/{path}/pulls/{number}"),
        &serde_json::json!({
            "number": number, "state": "open", "merged": false, "draft": true,
            "html_url": format!("https://github.example.com/Owner/Repo/pull/{number}"),
            "head": {"ref": source, "sha": sha, "repo": {"id": 9, "full_name": "Owner/Repo"}},
            "base": {"ref": target, "repo": {"id": 9, "full_name": "Owner/Repo"}},
            "merge_commit_sha": "ffffffffffffffffffffffffffffffffffffffff"
        }),
    );
}

#[test]
fn creates_a_gitlab_review_feature_on_the_real_source_branch() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    let master = fx.remote_sha("api-clone", "master");
    gitlab_mr(&fx, 486, "feature/x", "master", &sha);

    fx.hub(
        &hub,
        &[
            "feature",
            "start-review",
            "https://gitlab.example.com/G/Sub/Proj/-/merge_requests/486/?tab=diffs",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("api-review-486"))
    .stdout(predicate::str::contains(
        "https://gitlab.example.com/g/sub/proj/-/merge_requests/486",
    ))
    .stdout(predicate::str::contains(&sha))
    .stdout(predicate::str::contains("target master"))
    .stdout(predicate::str::contains(
        "Attach with: tmux attach -t acme-api_review_486",
    ));

    let f = fx.feature_json(&hub, "api-review-486");
    assert_eq!(f["checkout"], "acme-api_review_486");
    assert_eq!(f["status"], "open");
    let changes = f["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "only the URL's role takes part");
    let c = &changes[0];
    assert_eq!(c["role"], "api");
    assert_eq!(c["branch"], "feature/x");
    assert_eq!(c["stage"], "review");
    assert_eq!(
        c["review_url"],
        "https://gitlab.example.com/g/sub/proj/-/merge_requests/486"
    );
    assert_eq!(c["base"], "master");
    assert_eq!(c["base_sha"], master);
    assert_eq!(c["origin_seen"], true);
    assert_eq!(c["worktree"]["name"], "feature-x");
    assert_eq!(c["worktree"]["owner"], "hub");

    let wt = fx.worktree_dir.join("api-clone").join("feature-x");
    assert_eq!(
        fx.git(&wt, &["symbolic-ref", "--short", "HEAD"]),
        "feature/x"
    );
    assert_eq!(fx.git(&wt, &["rev-parse", "HEAD"]), sha);
    let hub_wt = fx.worktree_dir.join("acme").join("acme-api_review_486");
    assert_eq!(
        fx.git(&hub_wt, &["symbolic-ref", "--short", "HEAD"]),
        "api-review-486"
    );
    assert_eq!(
        fx.git(&hub, &["rev-parse", "api-review-486"]),
        fx.git(&hub, &["rev-parse", "main"])
    );
    let api = fx.project_home.join("api-clone");
    assert_eq!(
        fx.git(&api, &["symbolic-ref", "--short", "HEAD"]),
        "master",
        "main clone untouched"
    );
    assert!(!fx.worktree_dir.join("ui-clone").exists());
    assert!(!fx.worktree_dir.join("bff-clone").exists());

    assert_eq!(
        fx.window_names("acme-api_review_486"),
        vec!["hub", "ai", "api"]
    );
    assert!(!fx.tmux_log().contains("attach"));
    assert!(hub_wt.join("acme.code-workspace").exists());
    assert!(hub_wt.join(".claude/skills/hub/SKILL.md").exists());

    let log = fx.provider_log();
    assert!(
        log.contains(
            "glab api --hostname gitlab.example.com --method GET projects/g%2Fsub%2Fproj\n"
        ),
        "{log}"
    );
    assert!(log.contains("glab api --hostname gitlab.example.com --method GET projects/g%2Fsub%2Fproj/merge_requests/486\n"), "{log}");
    assert_eq!(
        log.lines().count(),
        4,
        "fetched once to plan, once to recheck before mutating:\n{log}"
    );
    assert!(!log.contains("gh "), "only the URL's provider CLI runs");
}

#[test]
fn creates_a_github_review_feature_as_a_sibling_with_custom_names() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "other", "--repo", "bff"])
        .assert()
        .success();
    let other_wt = fx.worktree_dir.join("acme").join("acme-other");
    fx.remote_branch("ui-clone", "topic");
    let sha = fx.remote_sha("ui-clone", "topic");
    github_pr(&fx, "owner/repo", 12, "topic", "develop", &sha);

    fx.hub(
        &other_wt,
        &[
            "feature",
            "start-review",
            "https://github.example.com/owner/repo/pull/12",
            "--name",
            "ui-pr-12",
            "--checkout",
            "custom",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "Attach with: tmux attach -t custom",
    ));

    let f = fx.feature_json(&hub, "ui-pr-12");
    assert_eq!(f["checkout"], "custom");
    assert_eq!(f["changes"][0]["role"], "ui");
    assert_eq!(f["changes"][0]["branch"], "topic");
    assert_eq!(
        f["changes"][0]["review_url"],
        "https://github.example.com/owner/repo/pull/12"
    );
    assert_eq!(f["changes"][0]["base"], "develop");
    let other = fx.feature_json(&hub, "other");
    assert_eq!(
        other["changes"].as_array().unwrap().len(),
        1,
        "the invoking feature is untouched"
    );
    assert!(fx.worktree_dir.join("acme").join("custom").is_dir());
    assert_eq!(fx.window_names("custom"), vec!["hub", "ai", "ui"]);
    let log = fx.provider_log();
    assert!(
        log.contains(
            "gh api --hostname github.example.com --method GET repos/owner/repo/pulls/12\n"
        ),
        "{log}"
    );
    assert!(!log.contains("glab"));
}

#[test]
fn records_the_review_target_as_base_even_when_it_is_not_the_role_base() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.git(&api, &["checkout", "-q", "-b", "release/1"]);
    fx.commit_file(&api, "release.txt", "release work");
    fx.git(&api, &["push", "-q", "origin", "release/1"]);
    fx.git(&api, &["checkout", "-q", "-b", "feature/x"]);
    fx.commit_file(&api, "x.txt", "feature work");
    fx.git(&api, &["push", "-q", "origin", "feature/x"]);
    fx.git(&api, &["checkout", "-q", "master"]);
    fx.git(&api, &["branch", "-D", "release/1", "feature/x"]);
    let sha = fx.remote_sha("api-clone", "feature/x");
    let release = fx.remote_sha("api-clone", "release/1");
    gitlab_mr(&fx, 7, "feature/x", "release/1", &sha);

    fx.hub(
        &hub,
        &[
            "feature",
            "start-review",
            "https://gitlab.example.com/g/sub/proj/-/merge_requests/7",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("target release/1"));
    let c = &fx.feature_json(&hub, "api-review-7")["changes"][0];
    assert_eq!(c["base"], "release/1");
    assert_eq!(
        c["base_sha"], release,
        "merge base against the MR target from the first save"
    );

    let json: serde_json::Value = serde_json::from_str(
        &String::from_utf8(
            fx.hub(&hub, &["status", "--json", "--feature", "api-review-7"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap(),
    )
    .unwrap();
    let row = &json["rows"][1];
    assert_eq!(row["role"], "api");
    assert_eq!(row["base"], "release/1");
    assert_eq!(row["custom_base"], true);
    assert_eq!(row["base_behind"], 0);
    assert_eq!(
        row["hint"],
        serde_json::Value::Null,
        "one commit of its own: not merged"
    );
}

#[test]
fn reuses_or_fast_forwards_an_unoccupied_local_source_branch() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.remote_branch("api-clone", "feature/equal");
    fx.remote_branch("api-clone", "feature/behind");
    let equal = fx.remote_sha("api-clone", "feature/equal");
    let behind = fx.remote_sha("api-clone", "feature/behind");
    fx.git(
        &api,
        &[
            "branch",
            "--no-track",
            "feature/equal",
            "origin/feature/equal",
        ],
    );
    fx.git(&api, &["branch", "--no-track", "feature/behind", "master"]);
    gitlab_mr(&fx, 1, "feature/equal", "master", &equal);
    fx.hub(
        &hub,
        &[
            "feature",
            "start-review",
            "https://gitlab.example.com/g/sub/proj/-/merge_requests/1",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Reusing branch feature/equal"));
    assert_eq!(fx.git(&api, &["rev-parse", "feature/equal"]), equal);

    gitlab_mr(&fx, 2, "feature/behind", "master", &behind);
    fx.hub(
        &hub,
        &[
            "feature",
            "start-review",
            "https://gitlab.example.com/g/sub/proj/-/merge_requests/2",
        ],
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Fast-forwarded feature/behind"));
    assert_eq!(fx.git(&api, &["rev-parse", "feature/behind"]), behind);
    assert_eq!(
        fx.git(
            &api,
            &[
                "for-each-ref",
                "--format=%(upstream)",
                "refs/heads/feature/behind"
            ]
        ),
        "",
        "tracking configuration is left as it was"
    );
    let wt = fx.worktree_dir.join("api-clone").join("feature-behind");
    assert_eq!(fx.git(&wt, &["rev-parse", "HEAD"]), behind);
    assert_eq!(fx.git(&api, &["worktree", "list"]).lines().count(), 3);
}

/// Local branches and worktrees of every clone, the hub's branches and
/// worktrees, feature records, and tmux sessions: what a refused command
/// must not change. Remote-tracking refs are deliberately excluded (a
/// fetch is allowed).
fn snapshot(fx: &Fixture, hub: &std::path::Path) -> String {
    let mut out = String::new();
    for dir in [
        hub.to_path_buf(),
        fx.project_home.join("api-clone"),
        fx.project_home.join("ui-clone"),
        fx.project_home.join("bff-clone"),
    ] {
        out.push_str(&fx.git(
            &dir,
            &["branch", "--list", "--format=%(refname) %(objectname)"],
        ));
        out.push('\n');
        out.push_str(&fx.git(&dir, &["worktree", "list", "--porcelain"]));
        out.push('\n');
    }
    let mut names: Vec<String> = std::fs::read_dir(hub.join(".git/hub/features"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    out.push_str(&names.join(","));
    let mut sessions: Vec<String> = std::fs::read_dir(&fx.tmux_state)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    sessions.sort();
    out.push_str(&sessions.join(","));
    out
}

/// Run `start-review` for `url`, expect exit `code` with `message` on
/// stderr, and require the snapshot to be unchanged.
fn refused(fx: &Fixture, hub: &std::path::Path, url: &str, code: i32, message: &str) {
    refused_from(fx, hub, hub, url, code, message);
}

/// `refused`, but the command runs in `cwd` while the snapshot still covers
/// the hub's main clone.
fn refused_from(
    fx: &Fixture,
    hub: &std::path::Path,
    cwd: &std::path::Path,
    url: &str,
    code: i32,
    message: &str,
) {
    let before = snapshot(fx, hub);
    fx.hub(cwd, &["feature", "start-review", url])
        .assert()
        .code(code)
        .stderr(predicate::str::contains(message));
    assert_eq!(
        snapshot(fx, hub),
        before,
        "refusal must not change local state"
    );
}

const MR: &str = "https://gitlab.example.com/g/sub/proj/-/merge_requests/";

#[test]
fn refuses_urls_that_do_not_resolve_to_one_role() {
    let (fx, hub) = world();
    refused(
        &fx,
        &hub,
        "https://gitlab.example.com/g/sub/proj",
        1,
        "unsupported review URL",
    );
    refused(
        &fx,
        &hub,
        "https://gitlab.example.com/other/proj/-/merge_requests/1",
        1,
        "no registered role",
    );
    assert_eq!(
        fx.provider_log(),
        "",
        "no provider call before a role matches"
    );
    fx.make_provider_repo("vap2-clone", "master", GITLAB_REMOTE);
    fx.add_repo(&hub, "vap2", "vap2-clone");
    let err_url = format!("{MR}1");
    fx.hub(&hub, &["feature", "start-review", &err_url])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("api, vap2"));
}

#[test]
fn refuses_ineligible_reviews_and_provider_failures() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    let closed = format!("{MR}2");
    gitlab_mr(&fx, 2, "feature/x", "master", &sha);
    let mut mr = serde_json::json!({
        "iid": 2, "state": "merged", "web_url": format!("{MR}2"),
        "source_project_id": 77, "target_project_id": 77,
        "source_branch": "feature/x", "target_branch": "master", "sha": sha
    });
    fx.provider_response("projects/g%2Fsub%2Fproj/merge_requests/2", &mr);
    refused(&fx, &hub, &closed, 1, "is merged");
    mr["state"] = "opened".into();
    mr["source_project_id"] = 78.into();
    fx.provider_response("projects/g%2Fsub%2Fproj/merge_requests/2", &mr);
    refused(&fx, &hub, &closed, 1, "fork");

    fx.remote_branch("ui-clone", "topic");
    let ui_sha = fx.remote_sha("ui-clone", "topic");
    github_pr(&fx, "Owner/Repo", 5, "topic", "develop", &ui_sha);
    let mut pr: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.provider_dir.join("repos__Owner__Repo__pulls__5.json"))
            .unwrap(),
    )
    .unwrap();
    pr["head"]["repo"] = serde_json::Value::Null;
    fx.provider_response("repos/Owner/Repo/pulls/5", &pr);
    refused(
        &fx,
        &hub,
        "https://github.example.com/Owner/Repo/pull/5",
        1,
        "no longer exists",
    );

    // Malformed JSON, an auth failure, and a missing CLI.
    std::fs::write(
        fx.provider_dir.join("projects__g_2Fsub_2Fproj.json"),
        "not json",
    )
    .unwrap();
    refused(&fx, &hub, &format!("{MR}2"), 1, "malformed JSON");
    let before = snapshot(&fx, &hub);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}2")])
        .env("FAKE_PROVIDER_FAIL", "HTTP 401: Unauthorized")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("401"))
        .stderr(predicate::str::contains(
            "glab auth status --hostname gitlab.example.com",
        ));
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}2")])
        .env("PATH", "/usr/bin:/bin")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("glab is not on PATH"));
    assert_eq!(snapshot(&fx, &hub), before);
}

#[test]
fn refuses_when_the_member_refs_do_not_match_the_review() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    let master = fx.remote_sha("api-clone", "master");
    gitlab_mr(&fx, 3, "feature/gone", "master", &sha);
    refused(
        &fx,
        &hub,
        &format!("{MR}3"),
        1,
        "origin/feature/gone not found",
    );
    gitlab_mr(&fx, 3, "feature/x", "release/none", &sha);
    refused(
        &fx,
        &hub,
        &format!("{MR}3"),
        1,
        "origin/release/none not found",
    );
    gitlab_mr(&fx, 3, "feature/x", "master", &master);
    refused(
        &fx,
        &hub,
        &format!("{MR}3"),
        1,
        "the fetch is stale or the branch moved",
    );
    let api = fx.project_home.join("api-clone");
    fx.git(
        &api,
        &[
            "remote",
            "set-url",
            "origin",
            "https://gitlab.example.com/g/sub/moved.git",
        ],
    );
    gitlab_mr(&fx, 3, "feature/x", "master", &sha);
    refused(&fx, &hub, &format!("{MR}3"), 1, "origin of");
    assert!(
        !fx.worktree_dir.join("api-clone").exists(),
        "no fallback branch from the base"
    );
}

#[test]
fn refuses_an_occupied_ahead_or_diverged_source_branch() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 4, "feature/x", "master", &sha);

    // The main clone holds the review's source branch only when that branch
    // is the role's base: any other branch checked out there is drift, which
    // exits 30 before the member checks run.
    fx.remote_branch("api-clone", "release/1");
    let master = fx.remote_sha("api-clone", "master");
    gitlab_mr(&fx, 9, "master", "release/1", &master);
    refused(
        &fx,
        &hub,
        &format!("{MR}9"),
        1,
        "checked out in the main clone",
    );

    fx.git(&api, &["branch", "feature/x", "origin/feature/x"]);
    let elsewhere = fx.worktree_dir.join("api-clone").join("by-hand");
    fx.git(
        &api,
        &[
            "worktree",
            "add",
            "-q",
            elsewhere.to_str().unwrap(),
            "feature/x",
        ],
    );
    refused(&fx, &hub, &format!("{MR}4"), 1, "checked out in a worktree");
    fx.git(&api, &["worktree", "remove", elsewhere.to_str().unwrap()]);

    fx.git(&api, &["checkout", "-q", "feature/x"]);
    let ahead = fx.commit_file(&api, "local.txt", "local work");
    fx.git(&api, &["checkout", "-q", "master"]);
    refused(&fx, &hub, &format!("{MR}4"), 1, "ahead of or has diverged");
    assert_eq!(
        fx.git(&api, &["rev-parse", "feature/x"]),
        ahead,
        "never reset"
    );
}

#[test]
fn refuses_name_branch_path_and_session_collisions() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 5, "feature/x", "master", &sha);
    let url = format!("{MR}5");

    fx.hub(
        &hub,
        &[
            "feature",
            "start",
            "api-review-5",
            "--checkout",
            "elsewhere",
        ],
    )
    .assert()
    .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "api-review-5"])
        .assert()
        .success();
    refused(&fx, &hub, &url, 1, "feature 'api-review-5' already exists");
    // A finished record is still a record: no reopening.
    assert_eq!(fx.feature_json(&hub, "api-review-5")["status"], "finished");

    gitlab_mr(&fx, 6, "feature/x", "master", &sha);
    let url = format!("{MR}6");
    fx.git(&hub, &["branch", "api-review-6"]);
    refused(&fx, &hub, &url, 1, "hub branch api-review-6 already exists");
    fx.git(&hub, &["branch", "-D", "api-review-6"]);
    fx.seed_session("acme-api_review_6", &[("hub", "/old")]);
    refused(
        &fx,
        &hub,
        &url,
        1,
        "tmux session acme-api_review_6 already exists",
    );
    std::fs::remove_dir_all(fx.tmux_state.join("acme-api_review_6")).unwrap();
    let occupied = fx.worktree_dir.join("api-clone").join("feature-x");
    std::fs::create_dir_all(&occupied).unwrap();
    refused(&fx, &hub, &url, 1, "already exists");
    std::fs::remove_dir_all(&occupied).unwrap();

    fx.hub(&hub, &["feature", "start-review", &url])
        .assert()
        .success();
    refused(&fx, &hub, &url, 1, "feature 'api-review-6' already exists");
}

#[test]
fn drift_exits_30_before_any_provider_call_but_a_dirty_unrelated_member_is_fine() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 8, "feature/x", "master", &sha);
    let bff = fx.project_home.join("bff-clone");
    fx.git(&bff, &["checkout", "-q", "-b", "other"]);
    refused(&fx, &hub, &format!("{MR}8"), 30, "drift");
    assert_eq!(fx.provider_log(), "");
    fx.git(&bff, &["checkout", "-q", "develop"]);

    std::fs::write(bff.join("scratch.txt"), "uncommitted").unwrap();
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}8")])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(bff.join("scratch.txt")).unwrap(),
        "uncommitted",
        "unrelated dirty checkouts are preserved"
    );
}

/// The other half of the exit-30 row: drift in the feature the command is
/// invoked from, which only `hub.cwd_feature` sees.
#[test]
fn drift_in_the_invoking_feature_exits_30_before_any_provider_call() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 9, "feature/x", "master", &sha);
    fx.hub(&hub, &["feature", "start", "feat-1", "--repo", "bff"])
        .assert()
        .success();
    let hub_wt = fx.worktree_dir.join("acme").join("acme-feat_1");
    assert!(hub_wt.is_dir(), "{}", hub_wt.display());
    let member = fx.worktree_dir.join("bff-clone").join("feat-1");
    assert!(member.is_dir(), "{}", member.display());
    // The member worktree directory is gone from where the record says it
    // is, so `status` for feat-1 reports it missing: structural drift.
    std::fs::rename(&member, member.with_file_name("moved-away")).unwrap();

    let log = fx.provider_log();
    refused_from(&fx, &hub, &hub_wt, &format!("{MR}9"), 30, "drift");
    assert_eq!(
        fx.provider_log(),
        log,
        "drift stops the command before any provider call"
    );
}

#[test]
fn a_failed_record_write_rolls_back_created_refs_and_restores_a_fast_forward() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.remote_branch("api-clone", "feature/new");
    fx.remote_branch("api-clone", "feature/behind");
    let new = fx.remote_sha("api-clone", "feature/new");
    let behind = fx.remote_sha("api-clone", "feature/behind");
    let master = fx.git(&api, &["rev-parse", "master"]);
    fx.git(&api, &["branch", "--no-track", "feature/behind", "master"]);
    fx.block_feature_writes(&hub);

    gitlab_mr(&fx, 1, "feature/new", "master", &new);
    let before = snapshot(&fx, &hub);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}1")])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Permission denied"));
    assert_eq!(
        snapshot(&fx, &hub),
        before,
        "created branch and worktrees are gone"
    );
    assert_eq!(fx.git(&api, &["branch", "--list", "feature/new"]), "");
    assert!(
        !fx.worktree_dir
            .join("acme")
            .join("acme-api_review_1")
            .exists()
    );
    assert!(!fx.has_session("acme-api_review_1"));

    gitlab_mr(&fx, 2, "feature/behind", "master", &behind);
    let before = snapshot(&fx, &hub);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}2")])
        .assert()
        .code(1);
    assert_eq!(snapshot(&fx, &hub), before);
    assert_eq!(
        fx.git(&api, &["rev-parse", "feature/behind"]),
        master,
        "fast-forward undone"
    );

    fx.allow_feature_writes(&hub);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}2")])
        .assert()
        .success();
    assert_eq!(fx.git(&api, &["rev-parse", "feature/behind"]), behind);
}

#[test]
fn metadata_that_changes_between_check_and_mutation_is_refused() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 3, "feature/x", "master", &sha);
    fx.provider_response_next(
        "projects/g%2Fsub%2Fproj/merge_requests/3",
        &serde_json::json!({
            "iid": 3, "state": "opened", "web_url": format!("{MR}3"),
            "source_project_id": 77, "target_project_id": 77,
            "source_branch": "feature/x", "target_branch": "develop", "sha": sha
        }),
    );
    refused(&fx, &hub, &format!("{MR}3"), 1, "changed while checking");
    assert_eq!(fx.provider_log().lines().count(), 4);
}

#[test]
fn a_tmux_failure_after_the_save_is_a_repairable_warning() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 4, "feature/x", "master", &sha);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}4")])
        .env("FAKE_TMUX_FAIL", "new-session")
        .assert()
        .success()
        .stdout(
            predicate::str::contains("warning:")
                .and(predicate::str::contains("hub tmux --feature api-review-4")),
        );
    let c = &fx.feature_json(&hub, "api-review-4")["changes"][0];
    assert_eq!(c["stage"], "review");
    assert_eq!(c["review_url"], format!("{MR}4"));
    assert!(
        fx.worktree_dir.join("api-clone").join("feature-x").is_dir(),
        "not rolled back"
    );
    assert!(!fx.has_session("acme-api_review_4"));
    fx.hub(&hub, &["tmux", "--feature", "api-review-4"])
        .assert()
        .success();
    assert_eq!(
        fx.window_names("acme-api_review_4"),
        vec!["hub", "ai", "api"]
    );
}

/// A concurrent fetch can move `origin/<source>` between the member checks
/// and the branch creation, so the branch is created at the moved tip rather
/// than the reviewed head: setup refuses, and rollback must still delete the
/// branch it made. The hub's `post-checkout` hook stands in for that fetch —
/// `git worktree add` runs it after the checks and just before the member
/// branch is created.
#[test]
fn a_source_branch_that_moves_during_setup_is_refused_and_leaves_no_branch() {
    use std::os::unix::fs::PermissionsExt;
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.remote_branch("api-clone", "feature/x");
    fx.remote_branch("api-clone", "feature/later");
    let sha = fx.remote_sha("api-clone", "feature/x");
    let later = fx.remote_sha("api-clone", "feature/later");
    gitlab_mr(&fx, 1, "feature/x", "master", &sha);
    let hook = hub.join(".git/hooks/post-checkout");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\ngit --git-dir={}/.git update-ref refs/remotes/origin/feature/x {later}\n",
            api.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    let before = snapshot(&fx, &hub);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}1")])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("moved during setup"));
    assert_eq!(
        fx.git(&api, &["branch", "--list", "feature/x"]),
        "",
        "the branch created at the moved tip is rolled back"
    );
    assert_eq!(snapshot(&fx, &hub), before);
}

#[test]
fn status_json_reports_the_saved_review_url_and_head() {
    let (fx, hub) = world();
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 9, "feature/x", "master", &sha);
    fx.hub(&hub, &["feature", "start-review", &format!("{MR}9")])
        .assert()
        .success();
    let json: serde_json::Value = serde_json::from_str(
        &String::from_utf8(
            fx.hub(&hub, &["status", "--json", "--feature", "api-review-9"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(json["checkout"], "acme-api_review_9");
    assert_eq!(json["drift"], false);
    let row = &json["rows"][1];
    assert_eq!(row["role"], "api");
    assert_eq!(row["stage"], "review");
    assert_eq!(row["review_url"], format!("{MR}9"));
    assert_eq!(row["ahead"], 0);
    assert_eq!(row["behind"], 0);
    assert_eq!(row["base"], "master");
    assert_eq!(row["custom_base"], false);
    let path = std::path::PathBuf::from(row["path"].as_str().unwrap());
    assert_eq!(fx.git(&path, &["rev-parse", "HEAD"]), sha);
    assert_eq!(
        json["rows"][0]["review_url"],
        serde_json::Value::Null,
        "hub row"
    );
}

#[test]
fn tmux_off_skips_the_session_collision_check() {
    let (fx, hub) = world();
    fx.write_config("tmux = false\n");
    fx.remote_branch("api-clone", "feature/x");
    let sha = fx.remote_sha("api-clone", "feature/x");
    gitlab_mr(&fx, 486, "feature/x", "master", &sha);
    // A stale session of the same name would block this with tmux on.
    fx.seed_session("acme-api_review_486", &[("hub", "/tmp")]);
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
    .stdout(predicate::str::contains("tmux is off; no session created"));
}
