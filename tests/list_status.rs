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

fn stdout(fx: &Fixture, dir: &std::path::Path, args: &[&str]) -> String {
    String::from_utf8(fx.hub(dir, args).output().unwrap().stdout).unwrap()
}

#[test]
fn list_shows_open_features_first_with_role_stages() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "start", "aaa-done"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "finish", "--feature", "aaa-done"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["feature", "review", "ui", "--feature", "feat-1"])
        .assert()
        .success();
    // Finished features are hidden unless --all is passed.
    let text = stdout(&fx, &hub, &["feature", "list"]);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("FEATURE"));
    assert!(lines[1].starts_with("feat-1"), "{text}");
    assert!(lines[1].contains("api:merged") && lines[1].contains("ui:review"));
    assert_eq!(lines.len(), 2, "{text}");
    assert!(!text.contains("aaa-done"), "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["feature", "list", "--json"])).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["name"], "feat-1");
    assert_eq!(json[0]["roles"][0]["role"], "api");
    assert_eq!(json[0]["roles"][0]["stage"], "merged");

    let text = stdout(&fx, &hub, &["feature", "list", "--all"]);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[1].starts_with("feat-1"), "{text}");
    assert!(lines[2].starts_with("aaa-done") && lines[2].contains("finished"));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["feature", "list", "--all", "--json"])).unwrap();
    assert_eq!(json[0]["name"], "feat-1");
    assert_eq!(json[1]["name"], "aaa-done");
    assert_eq!(json[1]["status"], "finished");
}

#[test]
fn list_hints_at_all_when_only_finished_features_exist() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    let text = stdout(&fx, &hub, &["feature", "list"]);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("FEATURE"), "{text}");
    assert_eq!(
        lines[1], "no open features; pass --all to include finished ones",
        "{text}"
    );
    assert_eq!(lines.len(), 2, "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["feature", "list", "--json"])).unwrap();
    assert_eq!(json, serde_json::json!([]));
    let text = stdout(&fx, &hub, &["feature", "list", "--all"]);
    assert!(text.lines().nth(1).unwrap().starts_with("feat-1"), "{text}");
    assert!(!text.contains("no open features"), "{text}");
}

#[test]
fn status_is_clean_after_start() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    fx.hub(&hub_wt, &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Feature: feat-1\nStatus: open\ntmux Session: feat-1\n",
        ))
        .stdout(predicate::str::contains("ok"))
        .stdout(predicate::str::contains("run hub feature merged").not());
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(json["drift"], false);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows[0]["role"], "hub");
    assert_eq!(rows[0]["branch"], "feat-1");
    assert_eq!(rows[1]["role"], "api");
    assert_eq!(rows[1]["exists"], true);
    assert_eq!(rows[1]["registered"], true);
    assert_eq!(rows[1]["branch_matches"], true);
    assert_eq!(rows[1]["dirty"], false);
    assert_eq!(
        rows[1]["ahead"],
        serde_json::Value::Null,
        "no remote branch yet"
    );
    assert_eq!(
        rows[1]["hint"],
        serde_json::Value::Null,
        "a fresh branch sits on the base but is not merged"
    );
    assert_eq!(
        rows[0]["base"],
        serde_json::Value::Null,
        "hub row has no base"
    );
    assert_eq!(rows[1]["base"], "master");
    assert_eq!(rows[1]["base_behind"], 0);
    assert_eq!(json["warnings"].as_array().unwrap().len(), 0);
    assert!(
        rows[1]["path"]
            .as_str()
            .unwrap()
            .ends_with("api-clone/feat-1")
    );
}

#[test]
fn dirty_hub_worktrees_are_reported_without_counting_as_drift() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    std::fs::write(hub_wt.join("notes.md"), "work in progress\n").unwrap();

    let feature: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub_wt, &["status", "--json"])).unwrap();
    assert_eq!(feature["rows"][0]["dirty"], true);
    assert_eq!(feature["rows"][0]["drift"], false);
    assert_eq!(feature["drift"], false);

    std::fs::write(hub.join("notes.md"), "work in progress\n").unwrap();
    let base: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    assert_eq!(base["rows"][0]["dirty"], true);
    assert_eq!(base["rows"][0]["drift"], false);
    assert_eq!(base["drift"], false);
}

#[test]
fn status_from_the_hub_reports_base_checkouts() {
    let (fx, hub) = world();
    fx.hub(&hub, &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hub: acme (base)"))
        .stdout(predicate::str::contains("master"))
        .stdout(predicate::str::contains("develop"));

    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    assert_eq!(json["hub"], "acme");
    assert_eq!(json["drift"], false);
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows[0]["role"], "hub");
    assert_eq!(rows[0]["branch"], "main");
    assert_eq!(rows[0]["stage"], "base");
    assert_eq!(rows[1]["role"], "api");
    assert_eq!(rows[1]["branch"], "master");
    assert_eq!(rows[1]["branch_matches"], true);
    assert_eq!(rows[2]["role"], "ui");
    assert_eq!(rows[2]["branch"], "develop");
    assert_eq!(rows[2]["branch_matches"], true);
}

#[test]
fn base_status_reports_dirty_without_drift() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    std::fs::write(api.join("scratch.txt"), "x").unwrap();
    fx.hub(&hub, &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dirty"));
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    assert_eq!(json["rows"][1]["dirty"], true);
    assert_eq!(json["rows"][1]["drift"], false);
    assert_eq!(json["drift"], false);
    std::fs::remove_file(api.join("scratch.txt")).unwrap();
}

#[test]
fn base_status_reports_structural_drift_with_exit_30() {
    let (fx, hub) = world();
    let ui = fx.project_home.join("ui-clone");
    fx.git(&ui, &["checkout", "-q", "-b", "other"]);
    fx.hub(&hub, &["status"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains("wrong-branch"));
}

#[test]
fn feature_status_reports_dirty_without_drift() {
    let (fx, hub) = world();
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    std::fs::write(ui_wt.join("scratch.txt"), "x").unwrap();
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dirty"));
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    let ui_row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["role"] == "ui")
        .unwrap();
    assert_eq!(ui_row["dirty"], true);
    assert_eq!(ui_row["drift"], false);
    assert_eq!(json["drift"], false);
}

#[test]
fn status_reports_structural_drift_with_exit_30() {
    let (fx, hub) = world();
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    let ui = fx.project_home.join("ui-clone");
    fx.git(
        &ui,
        &["worktree", "remove", "--force", ui_wt.to_str().unwrap()],
    );
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains("missing"));

    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success();
    let api = fx.project_home.join("api-clone");
    fx.git(&api, &["checkout", "-q", "-b", "feat-1-2"]);
    fx.hub(
        &hub,
        &[
            "feature",
            "add",
            "api",
            "--branch",
            "feat-1-2",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    fx.git(&api, &["checkout", "-q", "master"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains("wrong-branch"));
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows[1]["stage"], "merged");
    assert_eq!(rows[1]["drift"], false, "merged rows never drift");
    assert_eq!(rows[3]["role"], "api");
    assert_eq!(rows[3]["branch_matches"], false);
}

#[test]
fn base_status_can_be_requested_from_a_feature_worktree() {
    let (fx, _hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub_wt, &["status", "--base", "--json"])).unwrap();
    assert_eq!(json["hub"], "acme");
    assert_eq!(json["rows"][1]["role"], "api");
    assert_eq!(json["rows"][1]["stage"], "base");
    assert_eq!(json["rows"][2]["role"], "ui");
    assert_eq!(json["rows"][2]["stage"], "base");
}

#[test]
fn status_hints_when_a_branch_landed_on_origin() {
    let (fx, hub) = world();
    // Fast-forward: origin/master becomes the branch tip.
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.commit_file(&api_wt, "work.txt", "work");
    fx.git(&api_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.git(&api_wt, &["push", "-q", "origin", "feat-1:master"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "merged into origin/master; run hub feature merged api",
        ))
        .stdout(predicate::str::contains("run hub feature merged ui").not());

    // Merge commit: the branch is an ancestor of origin/develop but not its tip.
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    fx.commit_file(&ui_wt, "work.txt", "work");
    fx.git(&ui_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    let ui = fx.project_home.join("ui-clone");
    fx.git(&ui, &["fetch", "-q", "origin"]);
    fx.git(
        &ui,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge feat-1",
            "origin/feat-1",
        ],
    );
    fx.git(&ui, &["push", "-q", "origin", "develop"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "merged into origin/develop; run hub feature merged ui",
        ));
}

#[test]
fn status_hints_when_a_branch_vanished_from_origin() {
    let (fx, hub) = world();
    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    fx.commit_file(&ui_wt, "work.txt", "work");
    fx.git(&ui_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.hub(&hub, &["feature", "review", "ui", "--feature", "feat-1"])
        .assert()
        .success();
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(json["rows"][2]["ahead"], 0);
    assert_eq!(json["rows"][2]["behind"], 0);
    assert_eq!(
        json["rows"][2]["hint"],
        serde_json::Value::Null,
        "pushed but not merged"
    );
    fx.git(&ui_wt, &["push", "-q", "origin", "--delete", "feat-1"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "gone from origin; run hub feature merged ui",
        ));
}

#[test]
fn status_hints_merge_when_a_reviewed_branch_is_pruned_at_the_base_tip() {
    let (fx, hub) = world();
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.commit_file(&api_wt, "work.txt", "work");
    fx.git(&api_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    // Recording a review while the branch is on origin sets origin_seen.
    fx.hub(
        &hub,
        &[
            "feature",
            "review",
            "api",
            "https://x/1",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    // Fast-forward merge: the branch tip lands on origin/master and the
    // platform then deletes the now-merged source branch.
    fx.git(&api_wt, &["push", "-q", "origin", "feat-1:master"]);
    let api_clone = fx.project_home.join("api-clone");
    fx.git(&api_clone, &["push", "-q", "origin", "--delete", "feat-1"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "merged into origin/master; run hub feature merged api",
        ))
        .stdout(predicate::str::contains("at base tip").not());
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(
        rows[1]["hint"],
        "merged into origin/master; run hub feature merged api"
    );
}

#[test]
fn status_survives_a_missing_member_clone() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.hub(
        &hub,
        &[
            "feature",
            "review",
            "api",
            "https://gitlab.example/mr/7",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    std::fs::remove_dir_all(&api).unwrap();
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .code(30)
        .stdout(predicate::str::contains("missing"))
        .stdout(predicate::str::contains("warning:").and(predicate::str::contains("api-clone")));
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
    let rows = json["rows"].as_array().unwrap();
    let api_row = rows.iter().find(|r| r["role"] == "api").unwrap();
    assert_eq!(api_row["exists"], false);
    assert_eq!(api_row["registered"], false);
    assert_eq!(api_row["branch_matches"], false);
    assert_eq!(api_row["dirty"], false);
    assert_eq!(api_row["hint"], serde_json::Value::Null);
    assert_eq!(
        api_row["review_url"], "https://gitlab.example/mr/7",
        "the review URL comes from the record, not the clone"
    );
    let ui_row = rows.iter().find(|r| r["role"] == "ui").unwrap();
    assert_eq!(ui_row["exists"], true, "the other repo's row still renders");
}

#[test]
fn status_warns_when_a_fetch_fails_and_still_reports() {
    let (fx, hub) = world();
    let api = fx.project_home.join("api-clone");
    fx.git(
        &api,
        &[
            "remote",
            "set-url",
            "origin",
            fx.root.join("missing.git").to_str().unwrap(),
        ],
    );
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning: fetch failed"))
        .stdout(predicate::str::contains("run hub feature merged").not());
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
    let api_row = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == "api")
        .unwrap();
    assert_eq!(
        api_row["base_behind"], 0,
        "the last fetched origin/master still answers"
    );
    assert_eq!(json["drift"], false);
}

/// Point `api`'s open change at `feature/canonical`, created on origin at
/// the current `master` tip so the branch starts level with it.
fn custom_base(fx: &Fixture, hub: &std::path::Path) {
    let api = fx.project_home.join("api-clone");
    fx.git(
        &api,
        &[
            "push",
            "-q",
            "origin",
            "master:refs/heads/feature/canonical",
        ],
    );
    fx.hub(
        hub,
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
    .success();
}

fn api_row(json: &serde_json::Value) -> &serde_json::Value {
    json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == "api")
        .unwrap()
}

#[test]
fn status_reports_lag_behind_a_custom_base_without_drift() {
    let (fx, hub) = world();
    custom_base(&fx, &hub);
    let api = fx.project_home.join("api-clone");
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.commit_file(&api_wt, "work.txt", "work");

    // Ahead of the base with nothing missing: not a problem.
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("behind base").not());
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(api_row(&json)["base"], "feature/canonical");
    assert_eq!(api_row(&json)["base_behind"], 0);
    assert_eq!(api_row(&json)["hint"], serde_json::Value::Null);

    // The base moves on by one commit.
    fx.commit_file(&api, "canon.txt", "canonical work");
    fx.git(&api, &["push", "-q", "origin", "master:feature/canonical"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "behind base feature/canonical by 1",
        ));
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    let row = api_row(&json);
    assert_eq!(row["base_behind"], 1);
    assert_eq!(row["drift"], false);
    assert_eq!(json["drift"], false);
    assert!(row.get("ahead").is_some() && row.get("behind").is_some());
    assert_eq!(
        row["ahead"],
        serde_json::Value::Null,
        "not pushed: unchanged"
    );
    let ui = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["role"] == "ui")
        .unwrap();
    assert_eq!(ui["base"], "develop");
    assert_eq!(ui["base_behind"], 0);
}

#[test]
fn origin_divergence_and_base_lag_are_reported_independently() {
    let (fx, hub) = world();
    custom_base(&fx, &hub);
    let api = fx.project_home.join("api-clone");
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.commit_file(&api_wt, "work.txt", "work");
    // -u points git's upstream at the feature's own origin branch; the
    // hub's base must not follow it.
    fx.git(&api_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.commit_file(&api_wt, "more.txt", "more");
    fx.commit_file(&api, "canon.txt", "canonical work");
    fx.git(&api, &["push", "-q", "origin", "master:feature/canonical"]);

    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+1/-0"))
        .stdout(predicate::str::contains(
            "behind base feature/canonical by 1",
        ));
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    let row = api_row(&json);
    assert_eq!(row["ahead"], 1);
    assert_eq!(row["behind"], 0);
    assert_eq!(row["base"], "feature/canonical");
    assert_eq!(row["base_behind"], 1);
    assert_eq!(row["hint"], serde_json::Value::Null);
}

#[test]
fn merge_hint_targets_the_custom_base_not_the_manifest_base() {
    let (fx, hub) = world();
    custom_base(&fx, &hub);
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.commit_file(&api_wt, "work.txt", "work");
    fx.git(&api_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.git(
        &api_wt,
        &["push", "-q", "origin", "feat-1:feature/canonical"],
    );
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "merged into origin/feature/canonical; run hub feature merged api",
        ))
        .stdout(predicate::str::contains("origin/master").not());
}

#[test]
fn legacy_records_without_base_fall_back_to_the_manifest_base() {
    let (fx, hub) = world();
    // Strip the field the way a hub before 0.7 wrote the file.
    let path = hub.join(".git/hub/features").join("feat-1.json");
    let mut f: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for change in f["changes"].as_array_mut().unwrap() {
        change.as_object_mut().unwrap().remove("base");
    }
    std::fs::write(&path, serde_json::to_string_pretty(&f).unwrap()).unwrap();

    let api = fx.project_home.join("api-clone");
    fx.commit_file(&api, "later.txt", "later");
    fx.git(&api, &["push", "-q", "origin", "master"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("behind base master by 1"));
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(api_row(&json)["base"], "master");
    assert_eq!(api_row(&json)["base_behind"], 1);

    let ui_wt = fx.worktree_dir.join("ui-clone").join("feat-1");
    fx.commit_file(&ui_wt, "work.txt", "work");
    fx.git(&ui_wt, &["push", "-q", "-u", "origin", "feat-1"]);
    fx.git(&ui_wt, &["push", "-q", "origin", "feat-1:develop"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "merged into origin/develop; run hub feature merged ui",
        ));
}

#[test]
fn a_missing_base_ref_is_a_warning_with_unknown_lag() {
    let (fx, hub) = world();
    custom_base(&fx, &hub);
    let api = fx.project_home.join("api-clone");
    fx.git(
        &api,
        &["push", "-q", "origin", "--delete", "feature/canonical"],
    );
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "warning: origin/feature/canonical not found",
        ))
        .stdout(predicate::str::contains("behind base").not());
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(api_row(&json)["base"], "feature/canonical");
    assert_eq!(api_row(&json)["base_behind"], serde_json::Value::Null);
    assert_eq!(api_row(&json)["drift"], false);
    assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn status_moves_merged_roles_to_a_completed_table() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "merged", "api", "--feature", "feat-1"])
        .assert()
        .success();
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success();
    let text = stdout(&fx, &hub, &["status", "--feature", "feat-1"]);
    let (open, done) = text
        .split_once("\n\nCompleted\n")
        .unwrap_or_else(|| panic!("no Completed section: {text}"));
    let open_lines: Vec<&str> = open.lines().collect();
    assert_eq!(open_lines[0], "Feature: feat-1", "{text}");
    assert_eq!(open_lines[1], "Status: open", "{text}");
    assert_eq!(open_lines[2], "tmux Session: feat-1", "{text}");
    assert!(
        open_lines[3].starts_with("ROLE") && open_lines[3].contains("PATH"),
        "{text}"
    );
    assert!(open_lines[4].starts_with("hub"), "{text}");
    assert!(open_lines[5].starts_with("ui"), "{text}");
    assert_eq!(open_lines.len(), 6, "{text}");
    let done_lines: Vec<&str> = done.lines().collect();
    assert!(
        done_lines[0].starts_with("ROLE")
            && done_lines[0].contains("STAGE")
            && !done_lines[0].contains("STATE")
            && !done_lines[0].contains("PATH"),
        "{text}"
    );
    assert!(
        done_lines[1].starts_with("api") && done_lines[1].contains("merged"),
        "{text}"
    );
    assert_eq!(done_lines.len(), 2, "{text}");
    assert!(!text.contains("missing"), "{text}");
}

#[test]
fn finished_feature_status_shows_only_the_completed_table() {
    let (fx, hub) = world();
    fx.hub(&hub, &["feature", "finish", "--feature", "feat-1"])
        .assert()
        .success();
    let text = stdout(&fx, &hub, &["status", "--feature", "feat-1"]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Feature: feat-1", "{text}");
    assert_eq!(lines[1], "Status: finished", "{text}");
    assert_eq!(lines[2], "tmux Session: feat-1", "{text}");
    assert_eq!(lines[3], "", "{text}");
    assert_eq!(lines[4], "Completed", "{text}");
    assert!(
        lines[5].starts_with("ROLE") && !lines[5].contains("STATE"),
        "{text}"
    );
    assert!(
        lines[6].starts_with("hub") && lines[6].contains("finished"),
        "{text}"
    );
    assert!(
        lines[7].starts_with("api") && lines[7].contains("working"),
        "{text}"
    );
    assert!(
        lines[8].starts_with("ui") && lines[8].contains("working"),
        "{text}"
    );
    assert_eq!(lines.len(), 9, "{text}");
    assert!(!text.contains("missing"), "{text}");
}

#[test]
fn status_paths_under_home_are_shown_with_a_tilde() {
    let (fx, hub) = world();
    let run = |args: &[&str]| {
        let out = fx.hub(&hub, args).env("HOME", &fx.root).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let text = run(&["status", "--feature", "feat-1"]);
    assert!(text.contains("~/worktrees/api-clone/feat-1"), "{text}");
    assert!(!text.contains(fx.root.to_str().unwrap()), "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&run(&["status", "--feature", "feat-1", "--json"])).unwrap();
    let path = json["rows"][1]["path"].as_str().unwrap();
    assert!(path.starts_with(fx.root.to_str().unwrap()), "{path}");
}

#[test]
fn status_notes_a_branch_synced_to_a_moved_base() {
    let (fx, hub) = world();
    // The base moves on origin after the branch was created.
    let api = fx.project_home.join("api-clone");
    fx.commit_file(&api, "base.txt", "base moves");
    fx.git(&api, &["push", "-q", "origin", "master"]);
    // The role branch is fast-forwarded to it with no commits of its own
    // and was never pushed: a sync, not a merge.
    let api_wt = fx.worktree_dir.join("api-clone").join("feat-1");
    fx.git(&api_wt, &["fetch", "-q", "origin"]);
    fx.git(&api_wt, &["merge", "-q", "--ff-only", "origin/master"]);
    fx.hub(&hub, &["status", "--feature", "feat-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "at base tip, no commits of its own",
        ))
        .stdout(predicate::str::contains("merged into").not());
}

#[test]
fn hub_rows_report_ahead_behind_against_origin() {
    let (fx, hub) = world();
    let hub_wt = fx.worktree_dir.join("acme").join("feat-1");
    let bare = fx.remotes.join("hub.git");
    fx.git(
        &fx.remotes,
        &["init", "-q", "--bare", "-b", "main", "hub.git"],
    );
    fx.git(&hub, &["remote", "add", "origin", bare.to_str().unwrap()]);
    fx.git(&hub, &["push", "-q", "-u", "origin", "main"]);
    fx.git(&hub_wt, &["push", "-q", "-u", "origin", "feat-1"]);

    // One local commit on the feature branch: ahead 1.
    fx.commit_file(&hub_wt, "plan.md", "plan");
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows[0]["role"], "hub");
    assert_eq!(rows[0]["ahead"], 1);
    assert_eq!(rows[0]["behind"], 0);
    assert_eq!(rows[0]["base_behind"], serde_json::Value::Null);
    assert_eq!(json["warnings"].as_array().unwrap().len(), 0);

    // A teammate pushes to main: the base hub row is behind 1.
    let other = fx.root.join("teammate-hub");
    fx.git(
        &fx.root,
        &["clone", "-q", bare.to_str().unwrap(), "teammate-hub"],
    );
    fx.commit_file(&other, "hub.json.note", "teammate change");
    fx.git(&other, &["push", "-q", "origin", "main"]);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows[0]["role"], "hub");
    assert_eq!(rows[0]["branch"], "main");
    assert_eq!(rows[0]["ahead"], 0);
    assert_eq!(rows[0]["behind"], 1);
    fx.hub(&hub, &["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+0/-1"));

    // A dead remote is a warning, not a failure.
    fx.git(
        &hub,
        &[
            "remote",
            "set-url",
            "origin",
            fx.remotes.join("gone.git").to_str().unwrap(),
        ],
    );
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    let warnings = json["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().starts_with("fetch failed in")
                && w.as_str()
                    .unwrap()
                    .contains("status uses the last fetched refs")),
        "{warnings:?}"
    );
}

#[test]
fn hub_rows_have_no_ahead_behind_without_a_remote() {
    let (fx, hub) = world();
    let json: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--feature", "feat-1", "--json"],
    ))
    .unwrap();
    assert_eq!(json["rows"][0]["ahead"], serde_json::Value::Null);
    assert_eq!(json["rows"][0]["behind"], serde_json::Value::Null);
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    assert_eq!(json["rows"][0]["ahead"], serde_json::Value::Null);
    assert_eq!(json["rows"][0]["behind"], serde_json::Value::Null);
    assert_eq!(json["warnings"].as_array().unwrap().len(), 0);
}

#[test]
fn status_json_carries_the_review_url_on_every_row_kind() {
    let (fx, hub) = world();
    let feature: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--json", "--feature", "feat-1"],
    ))
    .unwrap();
    for row in feature["rows"].as_array().unwrap() {
        assert_eq!(row["review_url"], serde_json::Value::Null, "{row}");
    }
    fx.hub(
        &hub,
        &[
            "feature",
            "review",
            "api",
            "https://gitlab.example/mr/1",
            "--feature",
            "feat-1",
        ],
    )
    .assert()
    .success();
    let feature: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--json", "--feature", "feat-1"],
    ))
    .unwrap();
    assert_eq!(feature["rows"][1]["role"], "api");
    assert_eq!(
        feature["rows"][1]["review_url"],
        "https://gitlab.example/mr/1"
    );
    assert_eq!(feature["rows"][2]["review_url"], serde_json::Value::Null);
    let base: serde_json::Value =
        serde_json::from_str(&stdout(&fx, &hub, &["status", "--json"])).unwrap();
    for row in base["rows"].as_array().unwrap() {
        assert_eq!(row["review_url"], serde_json::Value::Null, "{row}");
    }
    // A record written before the field existed still loads and reports null.
    let path = hub.join(".git/hub/features/feat-1.json");
    let mut record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record["changes"][0]
        .as_object_mut()
        .unwrap()
        .remove("review_url");
    std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
    let feature: serde_json::Value = serde_json::from_str(&stdout(
        &fx,
        &hub,
        &["status", "--json", "--feature", "feat-1"],
    ))
    .unwrap();
    assert_eq!(feature["rows"][1]["review_url"], serde_json::Value::Null);
}
