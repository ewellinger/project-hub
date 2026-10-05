mod common;

use common::Fixture;
use predicates::prelude::*;

#[test]
fn scaffolds_a_hub_and_commits() {
    let fx = Fixture::new();
    let dir = fx.project_home.join("acme");
    std::fs::create_dir_all(&dir).unwrap();
    fx.hub(&dir, &["init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created hub 'acme'"));
    for file in [
        "hub.json",
        "CLAUDE.md",
        ".gitignore",
        ".claude/skills/hub/SKILL.md",
        ".agents/skills/hub/SKILL.md",
    ] {
        assert!(dir.join(file).exists(), "{file}");
    }
    assert!(
        !dir.join("features").exists(),
        "records are not scaffolded on main"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("hub.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], "acme");
    assert!(manifest.get("checkout_template").is_none());
    assert_eq!(manifest["repos"].as_array().unwrap().len(), 0);
    assert_eq!(
        fx.git(&dir, &["log", "-1", "--format=%s"]),
        "hub: init acme"
    );
    assert_eq!(fx.git(&dir, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(fx.git(&dir, &["status", "--porcelain"]), "");
    assert!(!dir.join(".prettierrc.json").exists(), "no Prettier config");
    assert!(
        !dir.join(".git/hooks/pre-push").exists(),
        "no pre-push hook"
    );
    let committed = fx.git(&dir, &["show", "--name-only", "--format=", "HEAD"]);
    let mut files: Vec<&str> = committed.lines().collect();
    files.sort();
    assert_eq!(files, vec![".gitignore", "CLAUDE.md", "hub.json"]);
    let claude = std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    for (index, line) in claude.lines().enumerate() {
        assert!(
            line.chars().count() <= 100,
            "CLAUDE.md line {} exceeds the configured print width: {line}",
            index + 1
        );
    }
    assert!(claude.contains(".git/hub/"), "{claude}");
    assert!(!claude.contains("features/*.json"), "{claude}");
    assert!(claude.contains("may be committed and pushed"), "{claude}");
    assert!(!claude.contains("pre-push"), "{claude}");
    let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    for line in [
        "*.code-workspace",
        ".claude/skills/hub/",
        ".agents/skills/hub/",
    ] {
        assert!(
            gitignore.lines().any(|l| l == line),
            "{line} in:\n{gitignore}"
        );
    }
    assert!(
        claude.contains(".claude/skills/hub/") && claude.contains("do not edit it"),
        "CLAUDE.md names the generated skill:\n{claude}"
    );
}

#[test]
fn honors_name_and_checkout_template() {
    let fx = Fixture::new();
    let dir = fx.project_home.join("dir");
    std::fs::create_dir_all(&dir).unwrap();
    fx.hub(
        &dir,
        &[
            "init",
            "--name",
            "acme",
            "--checkout-template",
            "acme-{feature_snake}",
        ],
    )
    .assert()
    .success();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("hub.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], "acme");
    assert_eq!(manifest["checkout_template"], "acme-{feature_snake}");
}

#[test]
fn rejects_template_without_placeholder_and_bad_name() {
    let fx = Fixture::new();
    let dir = fx.project_home.join("acme");
    std::fs::create_dir_all(&dir).unwrap();
    fx.hub(&dir, &["init", "--checkout-template", "static"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("placeholder"));
    fx.hub(&dir, &["init", "--name", "bad.name"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("hub name"));
    assert!(!dir.join("hub.json").exists());
}

#[test]
fn refuses_existing_hub_and_wrong_branch() {
    let fx = Fixture::new();
    let dir = fx.init_hub("acme");
    fx.hub(&dir, &["init"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already a hub"))
        .stderr(predicate::str::contains("--force"));

    let wrong_branch = fx.project_home.join("wrong");
    std::fs::create_dir_all(&wrong_branch).unwrap();
    fx.git(&wrong_branch, &["init", "-q", "-b", "develop"]);
    fx.hub(&wrong_branch, &["init"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("'main'"));
}

#[test]
fn init_accepts_a_repo_with_a_remote() {
    let fx = Fixture::new();
    let dir = fx.project_home.join("shared");
    std::fs::create_dir_all(&dir).unwrap();
    fx.git(&dir, &["init", "-q", "-b", "main"]);
    fx.git(
        &dir,
        &["remote", "add", "origin", "git@example.com:shared.git"],
    );
    fx.hub(&dir, &["init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created hub 'shared'"));
    assert_eq!(fx.git(&dir, &["remote"]), "origin", "the remote is kept");
    assert!(dir.join("hub.json").exists());
}

#[test]
fn init_preserves_existing_files_and_staged_work_in_a_local_repo() {
    let fx = Fixture::new();
    let dir = fx.project_home.join("acme");
    std::fs::create_dir_all(&dir).unwrap();
    fx.git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("CLAUDE.md"), "# mine\n").unwrap();
    std::fs::write(dir.join(".prettierrc.json"), "{\"printWidth\":80}\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "node_modules/\n").unwrap();
    fx.git(&dir, &["add", "."]);
    fx.git(&dir, &["commit", "-q", "-m", "existing"]);
    std::fs::write(dir.join("notes.txt"), "wip\n").unwrap();
    fx.git(&dir, &["add", "notes.txt"]);

    let hook = dir.join(".git/hooks/pre-push");
    std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();

    fx.hub(&dir, &["init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CLAUDE.md exists"));
    assert_eq!(
        std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
        "# mine\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".prettierrc.json")).unwrap(),
        "{\"printWidth\":80}\n"
    );
    let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(ignore.starts_with("node_modules/\n"), "{ignore}");
    assert!(ignore.contains("*.code-workspace\n"));
    let committed = fx.git(&dir, &["show", "--name-only", "--format=", "HEAD"]);
    let mut files: Vec<&str> = committed.lines().collect();
    files.sort();
    assert_eq!(files, vec![".gitignore", "hub.json"]);
    assert_eq!(
        fx.git(&dir, &["diff", "--cached", "--name-only"]),
        "notes.txt",
        "staged work untouched"
    );
    assert_eq!(
        std::fs::read_to_string(&hook).unwrap(),
        "#!/bin/sh\nexit 0\n",
        "a user's hook is left alone"
    );
}

#[test]
fn a_hub_can_push_to_its_remote() {
    let fx = Fixture::new();
    let dir = fx.init_hub("acme");
    let bare = fx.remotes.join("hub.git");
    fx.git(
        &fx.remotes,
        &["init", "-q", "--bare", "-b", "main", "hub.git"],
    );
    fx.git(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    fx.git(&dir, &["push", "-q", "-u", "origin", "main"]);
    assert_eq!(
        fx.git(&bare, &["log", "-1", "--format=%s", "main"]),
        "hub: init acme"
    );
}

/// A hub created before the workspace ignore rule: old `.gitignore`, the
/// workspace file committed, plus a user's own file and staged work.
fn legacy_hub(fx: &Fixture) -> std::path::PathBuf {
    let dir = fx.init_hub("acme");
    std::fs::write(dir.join(".gitignore"), ".tmp/\n.DS_Store\n").unwrap();
    std::fs::write(dir.join("acme.code-workspace"), "{\"folders\":[]}\n").unwrap();
    std::fs::write(dir.join("notes.md"), "# notes\n").unwrap();
    fx.git(
        &dir,
        &["add", ".gitignore", "acme.code-workspace", "notes.md"],
    );
    fx.git(&dir, &["commit", "-q", "-m", "legacy"]);
    std::fs::write(dir.join("wip.txt"), "wip\n").unwrap();
    fx.git(&dir, &["add", "wip.txt"]);
    dir
}

#[test]
fn force_merges_gitignore_and_untracks_matching_files() {
    let fx = Fixture::new();
    let dir = legacy_hub(&fx);

    fx.hub(&dir, &["init", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Untracked acme.code-workspace"));

    let ignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(ignore.starts_with(".tmp/\n.DS_Store\n"), "{ignore}");
    for line in [
        "*.code-workspace",
        ".claude/skills/hub/",
        ".agents/skills/hub/",
    ] {
        assert!(ignore.lines().any(|l| l == line), "{line} in:\n{ignore}");
    }
    assert!(dir.join("acme.code-workspace").exists(), "kept on disk");
    let tracked = fx.git(&dir, &["ls-files"]);
    assert!(
        !tracked.lines().any(|l| l == "acme.code-workspace"),
        "{tracked}"
    );
    assert!(tracked.lines().any(|l| l == "notes.md"), "{tracked}");
    assert_eq!(
        fx.git(&dir, &["log", "-1", "--format=%s"]),
        "hub: refresh init files"
    );
    let committed = fx.git(&dir, &["show", "--name-only", "--format=", "HEAD"]);
    let mut files: Vec<&str> = committed.lines().collect();
    files.sort();
    assert_eq!(files, vec![".gitignore", "acme.code-workspace"]);
    assert_eq!(
        fx.git(&dir, &["status", "--porcelain"]),
        "A  wip.txt",
        "staged work untouched and the workspace file now ignored"
    );
}

#[test]
fn force_with_nothing_to_refresh_makes_no_commit() {
    let fx = Fixture::new();
    let dir = fx.init_hub("acme");
    let head = fx.git(&dir, &["rev-parse", "HEAD"]);

    fx.hub(&dir, &["init", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Nothing to refresh"));

    assert_eq!(fx.git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(fx.git(&dir, &["status", "--porcelain"]), "");
}

#[test]
fn force_rejects_manifest_options() {
    let fx = Fixture::new();
    let dir = fx.init_hub("acme");
    for args in [
        &["init", "--force", "--name", "other"][..],
        &["init", "--force", "--checkout-template", "x-{feature}"][..],
    ] {
        fx.hub(&dir, args)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("--force"));
    }
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("hub.json")).unwrap()).unwrap();
    assert_eq!(manifest["name"], "acme");
}

#[test]
fn force_removes_the_hub_hook_and_leaves_a_foreign_one() {
    let fx = Fixture::new();
    let dir = fx.init_hub("acme");
    let hook = dir.join(".git/hooks/pre-push");

    let stale = "#!/bin/sh\necho \"hub coordinator repos are local-only\" >&2\nexit 1\n";
    std::fs::write(&hook, stale).unwrap();
    fx.hub(&dir, &["init", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed .git/hooks/pre-push"));
    assert!(!hook.exists(), "the hub's old hook is deleted");

    let foreign = "#!/bin/sh\nexit 0\n";
    std::fs::write(&hook, foreign).unwrap();
    fx.hub(&dir, &["init", "--force"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pre-push").not());
    assert_eq!(std::fs::read_to_string(&hook).unwrap(), foreign);
}
