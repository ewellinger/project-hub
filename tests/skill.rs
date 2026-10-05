mod common;

use std::collections::BTreeSet;
use std::path::Path;

use common::Fixture;

/// Every `hub ...` command in the skill's table must exist: strip the
/// placeholders and flags, then ask the binary for its help.
#[test]
fn every_command_in_the_skill_table_exists() {
    let skill =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/skills/hub/SKILL.md"))
            .unwrap();
    let mut commands: BTreeSet<Vec<String>> = BTreeSet::new();
    for line in skill.lines().filter(|l| l.starts_with('|')) {
        for (i, span) in line.split('`').enumerate() {
            let Some(rest) = span.strip_prefix("hub ").filter(|_| i % 2 == 1) else {
                continue;
            };
            let words: Vec<String> = rest
                .split_whitespace()
                .take_while(|w| w.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
                .map(str::to_string)
                .collect();
            commands.insert(words);
        }
    }
    assert!(
        commands.len() >= 11,
        "found only {} commands in the table: {commands:?}",
        commands.len()
    );

    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    for words in &commands {
        let mut args: Vec<&str> = words.iter().map(String::as_str).collect();
        args.push("--help");
        fx.hub(&hub, &args)
            .assert()
            .success()
            .stdout(predicates::str::contains("Usage: hub"));
    }

    assert!(
        commands.contains(&vec!["feature".to_string(), "start-review".to_string()]),
        "the skill table must teach start-review: {commands:?}"
    );
    assert!(
        skill.contains("hub status --json --feature"),
        "verification step"
    );
    assert!(skill.contains("review_url"), "the JSON field agents verify");
    assert!(
        skill.contains("hub feature finish --feature"),
        "cleanup step"
    );
}

fn skill_files(dir: &Path) -> [std::path::PathBuf; 2] {
    [
        dir.join(".claude/skills/hub/SKILL.md"),
        dir.join(".agents/skills/hub/SKILL.md"),
    ]
}

#[test]
fn any_command_restores_the_skill_in_every_worktree() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.hub(&hub, &["feature", "start", "docs-only"])
        .assert()
        .success();
    let wt = fx.worktree_dir.join("acme").join("docs-only");
    let [root_claude, root_agents] = skill_files(&hub);
    let [wt_claude, wt_agents] = skill_files(&wt);

    std::fs::create_dir_all(root_claude.parent().unwrap()).unwrap();
    std::fs::write(&root_claude, "stale\n").unwrap();
    let _ = std::fs::remove_file(&wt_agents);
    let _ = std::fs::remove_dir_all(wt.join(".claude"));

    fx.hub(&hub, &["feature", "list"])
        .assert()
        .success()
        .stderr(predicates::str::is_empty());

    for path in [&root_claude, &root_agents, &wt_claude, &wt_agents] {
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            hub::skill::SKILL,
            "{}",
            path.display()
        );
    }
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    assert_eq!(fx.git(&wt, &["status", "--porcelain"]), "");
}

#[test]
fn a_hub_without_the_ignore_lines_is_still_clean() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    // A hub created before the skill existed: no .gitignore lines, no files.
    let kept: Vec<String> = std::fs::read_to_string(hub.join(".gitignore"))
        .unwrap()
        .lines()
        .filter(|l| !l.contains("skills/hub"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(hub.join(".gitignore"), kept.concat()).unwrap();
    fx.git(
        &hub,
        &["commit", "-q", "--allow-empty", "-am", "drop skill ignores"],
    );
    let exclude = hub.join(".git/info/exclude");
    let _ = std::fs::remove_file(&exclude);
    for path in skill_files(&hub) {
        let _ = std::fs::remove_file(path);
    }

    fx.hub(&hub, &["repo", "list"]).assert().success();

    for path in skill_files(&hub) {
        assert!(path.is_file(), "{}", path.display());
    }
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    let exclude = std::fs::read_to_string(&exclude).unwrap();
    assert!(exclude.contains(".claude/skills/hub/"), "{exclude}");
    assert!(exclude.contains(".agents/skills/hub/"), "{exclude}");
}

#[test]
fn a_worktree_removed_behind_gits_back_does_not_fail_the_command() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.hub(&hub, &["feature", "start", "gone"])
        .assert()
        .success();
    std::fs::remove_dir_all(fx.worktree_dir.join("acme").join("gone")).unwrap();
    fx.hub(&hub, &["feature", "list", "--json"])
        .assert()
        .success()
        .stderr(predicates::str::is_empty());
}

#[test]
fn init_installs_the_skill() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    for path in skill_files(&hub) {
        assert_eq!(std::fs::read_to_string(&path).unwrap(), hub::skill::SKILL);
    }
    assert_eq!(fx.git(&hub, &["status", "--porcelain"]), "");
    assert_eq!(
        fx.git(&hub, &["ls-files", ".claude", ".agents"]),
        "",
        "skill files are not tracked"
    );
}

#[test]
fn feature_start_installs_the_skill_in_the_new_worktree() {
    let fx = Fixture::new();
    let hub = fx.init_hub("acme");
    fx.hub(&hub, &["feature", "start", "docs-only"])
        .assert()
        .success();
    let wt = fx.worktree_dir.join("acme").join("docs-only");
    for path in skill_files(&wt) {
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            hub::skill::SKILL,
            "{}",
            path.display()
        );
    }
    assert_eq!(fx.git(&wt, &["status", "--porcelain"]), "");
}
