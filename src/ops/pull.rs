//! `hub pull`: bring each role's checkout up to date with its own copy on
//! origin, fast-forward only. Nothing is merged, rebased, stashed, or
//! committed, and no hub state changes; a checkout that cannot simply move
//! forward is reported and left exactly as it was. Repos are pulled
//! concurrently, as `status` fetches them; checkouts of the same repo run
//! one after another, since concurrent fetches race on its ref locks.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::Result;
use crate::feature::Feature;
use crate::git::Git;
use crate::hub::Hub;
use crate::ops::session::{change_path, ordered_open_changes};
use crate::ops::status::parallel_map;

/// One checkout to fast-forward: the role named in the report line, the
/// repo it belongs to (the clone's directory name, which decides what may
/// run concurrently), the directory whose HEAD should be on `branch`, and
/// the branch whose `origin/` copy is pulled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub role: String,
    pub clone: String,
    pub path: PathBuf,
    pub branch: String,
}

/// Fast-forward every registered role's main clone on its base branch, in
/// manifest order. The hub's own clone is not pulled: `hub.json` on `main`
/// is pulled by hand, as the README says.
pub fn pull_base(hub: &Hub) -> Result<Vec<String>> {
    let targets: Vec<Target> = hub
        .manifest
        .repos
        .iter()
        .map(|repo| Target {
            role: repo.role.clone(),
            clone: repo.clone.clone(),
            path: hub.env.clone_path(&repo.clone),
            branch: repo.base.clone(),
        })
        .collect();
    if targets.is_empty() {
        return Ok(vec!["no repos registered; nothing to pull".into()]);
    }
    pull_all(targets)
}

/// Fast-forward every open change of `feature` on its branch, in manifest
/// order. Merged changes have no checkout of their own and are skipped.
pub fn pull_feature(hub: &Hub, feature: &Feature) -> Result<Vec<String>> {
    feature.require_open()?;
    let targets: Vec<Target> = ordered_open_changes(hub, feature)
        .into_iter()
        .map(|change| Target {
            role: change.role.clone(),
            clone: change.clone.clone(),
            path: change_path(hub, change),
            branch: change.branch.clone(),
        })
        .collect();
    if targets.is_empty() {
        return Ok(vec![format!(
            "feature '{}' has no open changes; nothing to pull",
            feature.name
        )]);
    }
    pull_all(targets)
}

/// Pull every target, one thread per repo, and return one line per target
/// in the order given.
fn pull_all(targets: Vec<Target>) -> Result<Vec<String>> {
    let groups = group_by_clone(targets);
    let mut indexed: Vec<(usize, Result<String>)> = parallel_map(&groups, |group| {
        group
            .iter()
            .map(|(index, target)| (*index, pull_one(target)))
            .collect::<Vec<_>>()
    })
    .into_iter()
    .flatten()
    .collect();
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, line)| line).collect()
}

/// Targets bucketed by repo, each tagged with its input position; buckets
/// and their members keep first-seen order.
fn group_by_clone(targets: Vec<Target>) -> Vec<Vec<(usize, Target)>> {
    let mut by_clone: HashMap<String, usize> = HashMap::new();
    let mut groups: Vec<Vec<(usize, Target)>> = Vec::new();
    for (index, target) in targets.into_iter().enumerate() {
        let group = *by_clone.entry(target.clone.clone()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push((index, target));
    }
    groups
}

/// One checkout: fetch, then fast-forward when it is on `branch`, clean,
/// and strictly behind `origin/<branch>`. Every other state is a line
/// saying what was found and that nothing was done. Only a git failure
/// that says nothing about the checkout (running git at all) is an error.
pub fn pull_one(target: &Target) -> Result<String> {
    let Target {
        role, path, branch, ..
    } = target;
    let git = Git::new(path);
    if !git.is_repo() {
        return Ok(format!("{role}: skipped, {} is missing", path.display()));
    }
    if let Err(err) = git.fetch("origin") {
        return Ok(format!("{role}: skipped, fetch failed: {err}"));
    }
    match git.current_branch()? {
        Some(current) if current == *branch => {}
        Some(current) => return Ok(format!("{role}: skipped, on {current} not {branch}")),
        None => {
            return Ok(format!(
                "{role}: skipped, detached HEAD instead of {branch}"
            ));
        }
    }
    let Some((ahead, behind)) = git.ahead_behind(branch)? else {
        return Ok(format!("{role}: skipped, {branch} has no origin/{branch}"));
    };
    if behind == 0 {
        return Ok(if ahead == 0 {
            format!("{role}: {branch} is up to date")
        } else {
            format!("{role}: {branch} is up to date, {ahead} ahead of origin")
        });
    }
    if ahead > 0 {
        return Ok(format!(
            "{role}: skipped, {branch} has diverged from origin/{branch} (+{ahead}/-{behind}); merge or rebase by hand"
        ));
    }
    if git.is_dirty()? {
        return Ok(format!(
            "{role}: skipped, {branch} is {behind} behind but the checkout is dirty"
        ));
    }
    match git.merge_ff_only(&format!("origin/{branch}")) {
        Ok(()) => Ok(format!(
            "{role}: {branch} fast-forwarded {behind} commit{}",
            if behind == 1 { "" } else { "s" }
        )),
        Err(err) => Ok(format!("{role}: fast-forward of {branch} failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use tempfile::TempDir;

    /// A bare `remote.git`, a `seed` clone that pushes to it, and the
    /// `clone` under test, both on `main`.
    struct World {
        _tmp: TempDir,
        seed: Git,
        clone: Git,
    }

    fn configure(git: &Git) {
        git.config("user.name", "Test").unwrap();
        git.config("user.email", "test@example.com").unwrap();
    }

    fn commit(git: &Git, name: &str) -> String {
        std::fs::write(git.dir().join(name), format!("{name}\n")).unwrap();
        git.add(&[Path::new(name)]).unwrap();
        git.commit(&format!("add {name}")).unwrap();
        git.rev_parse("HEAD").unwrap()
    }

    fn world() -> World {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let bare = root.join("remote.git");
        Git::new(&root)
            .raw(&["init", "-q", "--bare", "-b", "main", "remote.git"])
            .unwrap();
        let seed_dir = root.join("seed");
        std::fs::create_dir_all(&seed_dir).unwrap();
        Git::init(&seed_dir, "main").unwrap();
        let seed = Git::new(&seed_dir);
        configure(&seed);
        commit(&seed, "README.md");
        seed.raw(&["remote", "add", "origin", bare.to_str().unwrap()])
            .unwrap();
        seed.raw(&["push", "-q", "-u", "origin", "main"]).unwrap();
        Git::new(&root)
            .raw(&["clone", "-q", "remote.git", "clone"])
            .unwrap();
        let clone = Git::new(root.join("clone"));
        configure(&clone);
        World {
            _tmp: tmp,
            seed,
            clone,
        }
    }

    /// One more commit on `origin/main` that `clone` has not fetched.
    fn push(seed: &Git, name: &str) -> String {
        let sha = commit(seed, name);
        seed.raw(&["push", "-q", "origin", "main"]).unwrap();
        sha
    }

    fn target(git: &Git, branch: &str) -> Target {
        Target {
            role: "api".into(),
            clone: "api-clone".into(),
            path: git.dir().to_path_buf(),
            branch: branch.into(),
        }
    }

    #[test]
    fn targets_of_one_repo_share_a_group_and_lines_come_back_in_input_order() {
        let mk = |role: &str, clone: &str| Target {
            role: role.into(),
            clone: clone.into(),
            path: PathBuf::from("/nowhere").join(role),
            branch: "main".into(),
        };
        let groups = group_by_clone(vec![
            mk("api", "api-clone"),
            mk("ui", "ui-clone"),
            mk("vap2", "api-clone"),
        ]);
        let shape: Vec<Vec<(usize, &str)>> = groups
            .iter()
            .map(|g| g.iter().map(|(i, t)| (*i, t.role.as_str())).collect())
            .collect();
        assert_eq!(shape, vec![vec![(0, "api"), (2, "vap2")], vec![(1, "ui")]]);

        let lines = pull_all(vec![
            mk("api", "api-clone"),
            mk("ui", "ui-clone"),
            mk("vap2", "api-clone"),
        ])
        .unwrap();
        let roles: Vec<&str> = lines.iter().map(|l| l.split(':').next().unwrap()).collect();
        assert_eq!(roles, vec!["api", "ui", "vap2"], "{lines:?}");
        assert!(lines.iter().all(|l| l.contains("is missing")), "{lines:?}");
    }

    #[test]
    fn fast_forwards_a_clean_checkout_that_is_behind_origin() {
        let w = world();
        push(&w.seed, "a.txt");
        let tip = push(&w.seed, "b.txt");
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: main fast-forwarded 2 commits"
        );
        assert_eq!(w.clone.rev_parse("HEAD").unwrap(), tip);
        assert!(w.clone.dir().join("b.txt").is_file());
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: main is up to date"
        );
        push(&w.seed, "c.txt");
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: main fast-forwarded 1 commit"
        );
    }

    #[test]
    fn local_commits_are_reported_and_a_diverged_branch_is_left_alone() {
        let w = world();
        let local = commit(&w.clone, "local.txt");
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: main is up to date, 1 ahead of origin"
        );
        push(&w.seed, "a.txt");
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: skipped, main has diverged from origin/main (+1/-1); merge or rebase by hand"
        );
        assert_eq!(w.clone.rev_parse("HEAD").unwrap(), local);
        assert!(!w.clone.is_dirty().unwrap());
    }

    #[test]
    fn a_dirty_checkout_is_skipped_even_when_it_could_fast_forward() {
        let w = world();
        let before = w.clone.rev_parse("HEAD").unwrap();
        push(&w.seed, "a.txt");
        std::fs::write(w.clone.dir().join("scratch.txt"), "wip\n").unwrap();
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: skipped, main is 1 behind but the checkout is dirty"
        );
        assert_eq!(w.clone.rev_parse("HEAD").unwrap(), before);
        assert!(w.clone.dir().join("scratch.txt").is_file());
        assert!(
            w.clone.remote_branch_exists("main").unwrap(),
            "the fetch still happened"
        );
    }

    #[test]
    fn missing_repos_wrong_branches_and_unpushed_branches_are_skipped() {
        let w = world();
        let missing = Target {
            role: "ui".into(),
            clone: "ui-clone".into(),
            path: w.clone.dir().join("nope"),
            branch: "main".into(),
        };
        assert!(
            pull_one(&missing).unwrap().starts_with("ui: skipped, ")
                && pull_one(&missing).unwrap().ends_with("nope is missing")
        );

        w.clone.raw(&["checkout", "-q", "-b", "other"]).unwrap();
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: skipped, on other not main"
        );
        assert_eq!(
            pull_one(&target(&w.clone, "other")).unwrap(),
            "api: skipped, other has no origin/other"
        );
        w.clone.raw(&["checkout", "-q", "--detach"]).unwrap();
        assert_eq!(
            pull_one(&target(&w.clone, "main")).unwrap(),
            "api: skipped, detached HEAD instead of main"
        );
    }

    #[test]
    fn a_failed_fetch_is_reported_and_nothing_moves() {
        let w = world();
        let before = w.clone.rev_parse("HEAD").unwrap();
        push(&w.seed, "a.txt");
        w.clone
            .raw(&["remote", "set-url", "origin", "/nonexistent/remote.git"])
            .unwrap();
        let line = pull_one(&target(&w.clone, "main")).unwrap();
        assert!(line.starts_with("api: skipped, fetch failed: "), "{line}");
        assert_eq!(w.clone.rev_parse("HEAD").unwrap(), before);
    }
}
