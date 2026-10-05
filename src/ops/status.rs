use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::thread;

use comfy_table::{Cell, Color};
use crossterm::style::Stylize;
use serde::Serialize;

use crate::env::{home_dir, shorten_home};
use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, FeatureStatus};
use crate::git::{Git, same_path};
use crate::hub::Hub;
use crate::manifest::RepoSpec;
use crate::ops::session::change_path;
use crate::ops::styled_table;

#[derive(Debug, Serialize)]
pub struct StatusRow {
    pub role: String,
    pub branch: String,
    pub stage: String,
    pub path: PathBuf,
    pub exists: bool,
    pub registered: bool,
    pub branch_matches: bool,
    pub dirty: bool,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    /// Branch the change merges into; open changes only.
    pub base: Option<String>,
    /// `base` differs from the role's base in `hub.json`.
    pub custom_base: bool,
    /// Commits on `origin/<base>` the branch lacks; `None` when unknown.
    pub base_behind: Option<u64>,
    pub hint: Option<String>,
    /// Merge/pull request URL recorded on the change; `None` on hub and
    /// base rows and on changes without one.
    pub review_url: Option<String>,
    pub drift: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub feature: String,
    pub checkout: String,
    pub status: FeatureStatus,
    pub rows: Vec<StatusRow>,
    /// Non-fatal problems, such as a clone whose fetch failed.
    pub warnings: Vec<String>,
    pub drift: bool,
}

#[derive(Debug, Serialize)]
pub struct BaseStatusReport {
    pub hub: String,
    pub rows: Vec<StatusRow>,
    /// Non-fatal problems, such as a clone whose fetch failed.
    pub warnings: Vec<String>,
    pub drift: bool,
}

/// Compare the main hub and every registered repo's main clone with their
/// configured base branches. The hub's own origin is fetched too, when it
/// has one, alongside the repos' fetches.
pub fn base_status(hub: &Hub) -> Result<BaseStatusReport> {
    let mut warnings = Vec::new();
    let (hub_fetch, repo_results) = thread::scope(|scope| {
        let hub_handle = scope.spawn(|| fetch_hub(hub));
        let repo_results = parallel_map(&hub.manifest.repos, |repo| base_repo_row(hub, repo));
        (
            hub_handle.join().expect("hub fetch worker panicked"),
            repo_results,
        )
    });
    warnings.extend(hub_fetch?);
    let mut rows = vec![base_hub_row(hub)?];
    for result in repo_results {
        let (row, repo_warnings) = result?;
        rows.push(row);
        warnings.extend(repo_warnings);
    }
    let drift = rows.iter().any(|r| r.drift);
    Ok(BaseStatusReport {
        hub: hub.manifest.name.clone(),
        rows,
        warnings,
        drift,
    })
}

/// Fetch the hub's own origin when it has one, so the hub rows' ahead/behind
/// are current. Offline or a moved remote is a warning; the rows then use
/// whatever was fetched last. Returns the warnings instead of pushing into a
/// shared vector, so the caller can run this on its own thread alongside the
/// member fetches.
fn fetch_hub(hub: &Hub) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let git = hub.git();
    if git.has_remotes()?
        && let Err(err) = git.fetch("origin")
    {
        warnings.push(format!(
            "fetch failed in {}: {err}; status uses the last fetched refs",
            hub.root.display()
        ));
    }
    Ok(warnings)
}

fn base_hub_row(hub: &Hub) -> Result<StatusRow> {
    let git = hub.git();
    let dirty = git.is_dirty()?;
    let (ahead, behind) = match git.ahead_behind("main")? {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    Ok(StatusRow {
        role: "hub".into(),
        branch: "main".into(),
        stage: "base".into(),
        path: hub.root.clone(),
        exists: true,
        registered: true,
        branch_matches: true,
        dirty,
        ahead,
        behind,
        base: None,
        custom_base: false,
        base_behind: None,
        hint: None,
        review_url: None,
        drift: false,
    })
}

fn base_repo_row(hub: &Hub, repo: &RepoSpec) -> Result<(StatusRow, Vec<String>)> {
    let path = hub.env.clone_path(&repo.clone);
    let git = Git::new(&path);
    let mut warnings = Vec::new();
    if !git.is_repo() {
        warnings.push(format!(
            "clone missing: {}; skipping fetch for {}",
            path.display(),
            repo.role
        ));
        return Ok((
            StatusRow {
                role: repo.role.clone(),
                branch: repo.base.clone(),
                stage: "base".into(),
                path,
                exists: false,
                registered: false,
                branch_matches: false,
                dirty: false,
                ahead: None,
                behind: None,
                base: None,
                custom_base: false,
                base_behind: None,
                hint: None,
                review_url: None,
                drift: true,
            },
            warnings,
        ));
    }
    if let Err(err) = git.fetch("origin") {
        warnings.push(format!(
            "fetch failed in {}: {err}; status uses the last fetched refs",
            path.display()
        ));
    }
    let branch_matches = git.current_branch()?.as_deref() == Some(repo.base.as_str());
    let dirty = git.is_dirty()?;
    let (ahead, behind) = if git.local_branch_exists(&repo.base)? {
        match git.ahead_behind(&repo.base)? {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    Ok((
        StatusRow {
            role: repo.role.clone(),
            branch: repo.base.clone(),
            stage: "base".into(),
            path,
            exists: true,
            registered: true,
            branch_matches,
            dirty,
            ahead,
            behind,
            base: None,
            custom_base: false,
            base_behind: None,
            hint: None,
            review_url: None,
            drift: !branch_matches,
        },
        warnings,
    ))
}

/// Compare the feature's recorded state against the repos, including how
/// far each open branch is behind its base. Changes no hub state, worktree,
/// or branch; it does `git fetch --prune origin` in each clone with an open
/// change so the merge hints and base lag see current remote refs, and it
/// fetches the hub's own origin too when it has one. A failed fetch
/// (offline, moved remote) is a warning, and both then use whatever was
/// fetched last.
pub fn status(hub: &Hub, feature: &Feature) -> Result<StatusReport> {
    let mut warnings = Vec::new();
    let groups = group_changes_by_clone(&feature.changes);
    let (hub_fetch, group_results) = thread::scope(|scope| {
        let hub_handle = scope.spawn(|| fetch_hub(hub));
        let group_results = parallel_map(&groups, |group| {
            let mut fetched = HashSet::new();
            let mut results = Vec::new();
            for &(index, change) in group {
                let mut change_warnings = Vec::new();
                let result = change_row(hub, feature, change, &mut fetched, &mut change_warnings)
                    .map(|row| (row, change_warnings));
                let failed = result.is_err();
                results.push((index, result));
                if failed {
                    break;
                }
            }
            results
        });
        (
            hub_handle.join().expect("hub fetch worker panicked"),
            group_results,
        )
    });
    warnings.extend(hub_fetch?);
    let mut rows = vec![hub_row(hub, feature)?];
    let mut indexed_results: Vec<_> = group_results.into_iter().flatten().collect();
    indexed_results.sort_by_key(|(index, _)| *index);
    for (_, result) in indexed_results {
        let (row, change_warnings) = result?;
        rows.push(row);
        warnings.extend(change_warnings);
    }
    let drift = rows.iter().any(|r| r.drift);
    Ok(StatusReport {
        feature: feature.name.clone(),
        checkout: feature.checkout.clone(),
        status: feature.status,
        rows,
        warnings,
        drift,
    })
}

fn group_changes_by_clone(changes: &[Change]) -> Vec<Vec<(usize, &Change)>> {
    // Follow-up changes can share a clone. Keep those operations sequential
    // while allowing different clones to fetch and inspect concurrently.
    let mut group_indexes: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<Vec<(usize, &Change)>> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        let group_index = match group_indexes.get(change.clone.as_str()) {
            Some(index) => *index,
            None => {
                let index = groups.len();
                group_indexes.insert(change.clone.as_str(), index);
                groups.push(Vec::new());
                index
            }
        };
        groups[group_index].push((index, change));
    }
    groups
}

/// Run `f` over `items` on one scoped thread each and return the results
/// in input order. Shared with `pull`.
pub(crate) fn parallel_map<T, U, F>(items: &[T], f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Sync,
{
    thread::scope(|scope| {
        let handles: Vec<_> = items.iter().map(|item| scope.spawn(|| f(item))).collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("status worker panicked"))
            .collect()
    })
}

fn hub_row(hub: &Hub, feature: &Feature) -> Result<StatusRow> {
    let path = hub.hub_worktree_path(&feature.checkout);
    let exists = path.is_dir();
    let entry = hub
        .git()
        .worktrees()?
        .into_iter()
        .find(|w| same_path(&w.path, &path));
    let registered = entry.is_some();
    let branch_matches = entry
        .as_ref()
        .is_some_and(|w| w.branch.as_deref() == Some(feature.name.as_str()));
    let dirty = exists && registered && Git::new(&path).is_dirty()?;
    let (ahead, behind) = if exists && registered && branch_matches {
        match hub.git().ahead_behind(&feature.name)? {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let open = feature.is_open();
    Ok(StatusRow {
        role: "hub".into(),
        branch: feature.name.clone(),
        stage: if open {
            "open".into()
        } else {
            "finished".into()
        },
        path,
        exists,
        registered,
        branch_matches,
        dirty,
        ahead,
        behind,
        base: None,
        custom_base: false,
        base_behind: None,
        hint: None,
        review_url: None,
        drift: open && (!exists || !registered || !branch_matches),
    })
}

fn change_row(
    hub: &Hub,
    feature: &Feature,
    change: &Change,
    fetched: &mut HashSet<String>,
    warnings: &mut Vec<String>,
) -> Result<StatusRow> {
    // Paths come from the change itself, so rows for a role that has since
    // been removed from hub.json still render; only the hint needs the manifest.
    let clone_dir = hub.env.clone_path(&change.clone);
    let git = Git::new(&clone_dir);
    let path = change_path(hub, change);
    let open = change.is_open() && feature.is_open();
    if !git.is_repo() {
        warnings.push(format!(
            "clone missing: {}; skipping fetch and hint for {}",
            clone_dir.display(),
            change.role
        ));
        return Ok(StatusRow {
            role: change.role.clone(),
            branch: change.branch.clone(),
            stage: change.stage.as_str().to_string(),
            path,
            exists: false,
            registered: false,
            branch_matches: false,
            dirty: false,
            ahead: None,
            behind: None,
            base: None,
            custom_base: false,
            base_behind: None,
            hint: None,
            review_url: change.review_url.clone(),
            drift: open,
        });
    }
    if open
        && fetched.insert(change.clone.clone())
        && let Err(err) = git.fetch("origin")
    {
        warnings.push(format!(
            "fetch failed in {}: {err}; hints use the last fetched refs",
            clone_dir.display()
        ));
    }
    let exists = path.is_dir();
    let (registered, branch_matches) = match &change.worktree {
        None => (
            true,
            git.current_branch()?.as_deref() == Some(change.branch.as_str()),
        ),
        Some(_) => match git
            .worktrees()?
            .into_iter()
            .find(|w| same_path(&w.path, &path))
        {
            Some(w) => (true, w.branch.as_deref() == Some(change.branch.as_str())),
            None => (false, false),
        },
    };
    let dirty = exists && registered && Git::new(&path).is_dirty()?;
    let (ahead, behind) = if git.local_branch_exists(&change.branch)? {
        match git.ahead_behind(&change.branch)? {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let base = if open {
        change.effective_base(&hub.manifest)
    } else {
        None
    };
    let custom_base =
        base.is_some() && base != hub.manifest.repo(&change.role).map(|r| r.base.clone());
    let base_behind = match &base {
        Some(b) if git.local_branch_exists(&change.branch)? => {
            let base_ref = format!("origin/{b}");
            if git.remote_branch_exists(b)? {
                Some(git.missing_commits(&change.branch, &base_ref)?)
            } else {
                warnings.push(format!(
                    "{base_ref} not found in {}; base lag for {} is unknown",
                    clone_dir.display(),
                    change.role
                ));
                None
            }
        }
        _ => None,
    };
    let hint = if open {
        merge_hint(&git, change, base.as_deref())?
    } else {
        None
    };
    Ok(StatusRow {
        role: change.role.clone(),
        branch: change.branch.clone(),
        stage: change.stage.as_str().to_string(),
        path,
        exists,
        registered,
        branch_matches,
        dirty,
        ahead,
        behind,
        base,
        custom_base,
        base_behind,
        hint,
        review_url: change.review_url.clone(),
        drift: open && (!exists || !registered || !branch_matches),
    })
}

/// Hint 1: the branch reached `origin/<base>` and has work beyond the base
/// commit it started from. A fresh branch is an ancestor of the base too, so
/// ancestry alone would flag every untouched branch as merged. A branch
/// fast-forwarded to a moved base also looks merged by history alone; the
/// sync case is a branch that is at the base tip, has no `origin/<branch>`
/// now, and was never seen on origin by hub. That last condition is
/// not the same as "never pushed": a branch pushed by hand with no review
/// recorded still has `origin_seen == false`, and a push-then-fast-forward-
/// then-prune of a branch hub never saw on origin still reads as a sync
/// rather than a merge. Hint 2: the branch was seen on origin and is gone
/// after a pruning fetch.
fn merge_hint(git: &Git, change: &Change, base: Option<&str>) -> Result<Option<String>> {
    if let (Some(base), Some(base_sha)) = (base, change.base_sha.as_deref()) {
        let base_ref = format!("origin/{base}");
        if git.local_branch_exists(&change.branch)?
            && git.is_ancestor(&change.branch, &base_ref)?
            && !git.is_ancestor(&change.branch, base_sha)?
        {
            let synced = git.rev_parse(&change.branch)? == git.rev_parse(&base_ref)?
                && !git.remote_branch_exists(&change.branch)?
                && !change.origin_seen;
            return Ok(Some(if synced {
                "at base tip, no commits of its own".to_string()
            } else {
                format!(
                    "merged into {base_ref}; run hub feature merged {}",
                    change.role
                )
            }));
        }
    }
    if change.origin_seen && !git.remote_branch_exists(&change.branch)? {
        return Ok(Some(format!(
            "gone from origin; run hub feature merged {}",
            change.role
        )));
    }
    Ok(None)
}

pub fn render_table(report: &StatusReport, tmux_on: bool) -> Vec<String> {
    render_table_with_home(report, home_dir().as_deref(), tmux_on)
}

/// A merged change, or any row of a finished feature, is closed: its
/// worktree is gone by design, so it is listed under `Completed` without
/// state or path instead of showing as `missing`.
fn render_table_with_home(
    report: &StatusReport,
    home: Option<&Path>,
    tmux_on: bool,
) -> Vec<String> {
    let mut lines = header_lines(report, std::io::stdout().is_terminal(), tmux_on);
    let (closed, open): (Vec<&StatusRow>, Vec<&StatusRow>) = report
        .rows
        .iter()
        .partition(|r| report.status == FeatureStatus::Finished || r.stage == "merged");
    if !open.is_empty() {
        lines.extend(open_table(&open, home));
    }
    if !closed.is_empty() {
        lines.push(String::new());
        lines.push("Completed".into());
        lines.extend(styled_table(
            &["ROLE", "BRANCH", "STAGE"],
            closed
                .iter()
                .map(|r| {
                    vec![
                        Cell::new(&r.role),
                        Cell::new(&r.branch),
                        Cell::new(&r.stage),
                    ]
                })
                .collect(),
        ));
    }
    lines.extend(warning_lines(&report.warnings));
    lines
}

/// The header lines above the feature table; the session line only when
/// tmux is on. On a terminal the feature and session names are bold cyan so
/// they stand out from the labels; piped output is plain text.
fn header_lines(report: &StatusReport, terminal: bool, tmux_on: bool) -> Vec<String> {
    let status = match report.status {
        FeatureStatus::Open => "open",
        FeatureStatus::Finished => "finished",
    };
    let mut lines = vec![
        format!("Feature: {}", emphasise(&report.feature, terminal)),
        format!("Status: {status}"),
    ];
    if tmux_on {
        lines.push(format!(
            "tmux Session: {}",
            emphasise(&report.checkout, terminal)
        ));
    }
    lines
}

fn emphasise(text: &str, terminal: bool) -> String {
    if terminal {
        text.bold().cyan().to_string()
    } else {
        text.to_string()
    }
}

pub fn render_base_table(report: &BaseStatusReport) -> Vec<String> {
    let mut lines = vec![format!("Hub: {} (base)", report.hub)];
    let rows: Vec<&StatusRow> = report.rows.iter().collect();
    lines.extend(open_table(&rows, home_dir().as_deref()));
    lines.extend(warning_lines(&report.warnings));
    lines
}

fn warning_lines(warnings: &[String]) -> impl Iterator<Item = String> + '_ {
    warnings.iter().map(|w| format!("warning: {w}"))
}

/// The `BASE` column appears only when some row merges into a branch other
/// than its role's base; every role on its default base keeps the table
/// compact.
fn open_table(status_rows: &[&StatusRow], home: Option<&Path>) -> Vec<String> {
    let show_base = status_rows.iter().any(|r| r.custom_base);
    let mut headers = vec!["ROLE", "BRANCH"];
    if show_base {
        headers.push("BASE");
    }
    headers.extend(["STAGE", "STATE", "PATH"]);
    let rows: Vec<Vec<Cell>> = status_rows
        .iter()
        .map(|r| {
            let mut cells = vec![Cell::new(&r.role), Cell::new(&r.branch)];
            if show_base {
                cells.push(Cell::new(r.base.as_deref().unwrap_or("")));
            }
            cells.extend([
                Cell::new(&r.stage),
                state_cell(r),
                Cell::new(shorten_home(&r.path, home)),
            ]);
            cells
        })
        .collect();
    styled_table(&headers, rows)
}

/// Colour class of a status row's `state` cell, shared by the CLI table and
/// the dashboard so both render a row the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateColour {
    Red,
    Yellow,
    Green,
}

fn structural_drift(row: &StatusRow) -> bool {
    !row.exists || !row.registered || !row.branch_matches
}

fn diverged(row: &StatusRow) -> Option<(u64, u64)> {
    match (row.ahead, row.behind) {
        (Some(a), Some(b)) if a > 0 || b > 0 => Some((a, b)),
        _ => None,
    }
}

fn base_lag(row: &StatusRow) -> Option<(&str, u64)> {
    match (row.base.as_deref(), row.base_behind) {
        (Some(base), Some(n)) if n > 0 => Some((base, n)),
        _ => None,
    }
}

/// The `state` cell text: structural problem, dirtiness, divergence, base
/// lag, hint, joined with `, `; `ok` when none of the first four apply.
pub fn state_text(row: &StatusRow) -> String {
    let mut state = Vec::new();
    if !row.exists {
        state.push("missing".to_string());
    } else if !row.registered {
        state.push("unregistered".to_string());
    } else if !row.branch_matches {
        state.push("wrong-branch".to_string());
    }
    if row.dirty {
        state.push("dirty".to_string());
    }
    if let Some((a, b)) = diverged(row) {
        state.push(format!("+{a}/-{b}"));
    }
    if let Some((base, n)) = base_lag(row) {
        state.push(format!("behind base {base} by {n}"));
    }
    if state.is_empty() {
        state.push("ok".to_string());
    }
    if let Some(hint) = &row.hint {
        state.push(hint.clone());
    }
    state.join(", ")
}

/// Red for a structural problem, yellow for dirtiness, divergence, base
/// lag, or a hint, green otherwise.
pub fn state_colour(row: &StatusRow) -> StateColour {
    if structural_drift(row) {
        StateColour::Red
    } else if row.dirty || diverged(row).is_some() || base_lag(row).is_some() || row.hint.is_some()
    {
        StateColour::Yellow
    } else {
        StateColour::Green
    }
}

fn state_cell(row: &StatusRow) -> Cell {
    let color = match state_colour(row) {
        StateColour::Red => Color::Red,
        StateColour::Yellow => Color::Yellow,
        StateColour::Green => Color::Green,
    };
    Cell::new(state_text(row)).fg(color)
}

pub fn render_json(report: &StatusReport) -> Result<String> {
    serde_json::to_string_pretty(report)
        .map_err(|e| HubError::Precondition(format!("serializing status: {e}")))
}

pub fn render_base_json(report: &BaseStatusReport) -> Result<String> {
    serde_json::to_string_pretty(report)
        .map_err(|e| HubError::Precondition(format!("serializing status: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn report() -> StatusReport {
        StatusReport {
            feature: "feat-1".into(),
            checkout: "acme-feat_1".into(),
            status: FeatureStatus::Open,
            rows: vec![],
            warnings: vec![],
            drift: false,
        }
    }

    #[test]
    fn header_is_plain_text_when_piped() {
        assert_eq!(
            header_lines(&report(), false, true),
            vec![
                "Feature: feat-1",
                "Status: open",
                "tmux Session: acme-feat_1"
            ]
        );
    }

    #[test]
    fn header_emphasises_the_names_on_a_terminal() {
        let lines = header_lines(&report(), true, true);
        assert_eq!(lines[1], "Status: open");
        for (line, name) in [(&lines[0], "feat-1"), (&lines[2], "acme-feat_1")] {
            assert!(line.contains("\x1b[1m"), "{line:?} is not bold");
            assert!(line.contains(name), "{line:?}");
            assert!(line.ends_with("\x1b[0m"), "{line:?} leaves styling open");
        }
        assert!(lines[0].starts_with("Feature: \x1b["), "{:?}", lines[0]);
    }

    #[test]
    fn header_drops_the_session_line_when_tmux_is_off() {
        assert_eq!(
            header_lines(&report(), false, false),
            vec!["Feature: feat-1", "Status: open"]
        );
    }

    fn row() -> StatusRow {
        StatusRow {
            role: "api".into(),
            branch: "feature/example".into(),
            stage: "working".into(),
            path: "/tmp/api".into(),
            exists: true,
            registered: true,
            branch_matches: true,
            dirty: false,
            ahead: Some(0),
            behind: Some(0),
            base: None,
            custom_base: false,
            base_behind: None,
            hint: None,
            review_url: None,
            drift: false,
        }
    }

    #[test]
    fn open_table_adds_a_base_column_only_when_a_change_overrides_its_role_base() {
        let mut default = row();
        default.base = Some("develop".into());
        let mut custom = row();
        custom.role = "bff".into();
        custom.base = Some("feature/integration".into());
        custom.custom_base = true;

        let without = open_table(&[&default], None);
        assert!(!without[0].contains("BASE"), "{:?}", without[0]);
        assert!(
            !without.iter().any(|l| l.contains("develop")),
            "{without:#?}"
        );

        let with = open_table(&[&default, &custom], None);
        assert!(with[0].contains("BASE"), "{:?}", with[0]);
        assert!(with.iter().any(|l| l.contains("develop")), "{with:#?}");
        assert!(
            with.iter().any(|l| l.contains("feature/integration")),
            "{with:#?}"
        );
    }

    #[test]
    fn status_state_cells_use_severity_colors() {
        assert_eq!(state_cell(&row()), Cell::new("ok").fg(Color::Green));

        let mut dirty = row();
        dirty.dirty = true;
        assert_eq!(state_cell(&dirty), Cell::new("dirty").fg(Color::Yellow));

        let mut missing = row();
        missing.exists = false;
        assert_eq!(state_cell(&missing), Cell::new("missing").fg(Color::Red));
    }

    #[test]
    fn parallel_map_runs_concurrently_and_preserves_input_order() {
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let values = parallel_map(&[3, 2, 1], |value| {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(current, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20 * value));
            active.fetch_sub(1, Ordering::SeqCst);
            value * 2
        });

        assert_eq!(values, vec![6, 4, 2]);
        assert!(peak.load(Ordering::SeqCst) > 1);
    }

    fn test_row(
        exists: bool,
        dirty: bool,
        ahead: Option<u64>,
        behind: Option<u64>,
        hint: Option<&str>,
    ) -> StatusRow {
        StatusRow {
            role: "api".into(),
            branch: "feat-1".into(),
            stage: "working".into(),
            path: PathBuf::from("/p"),
            exists,
            registered: true,
            branch_matches: true,
            dirty,
            ahead,
            behind,
            base: None,
            custom_base: false,
            base_behind: None,
            hint: hint.map(str::to_string),
            review_url: None,
            drift: !exists,
        }
    }

    #[test]
    fn state_text_joins_every_condition_in_order() {
        assert_eq!(
            state_text(&test_row(true, false, Some(0), Some(0), None)),
            "ok"
        );
        assert_eq!(
            state_text(&test_row(false, false, None, None, None)),
            "missing"
        );
        assert_eq!(
            state_text(&test_row(
                true,
                true,
                Some(2),
                Some(1),
                Some("looks merged")
            )),
            "dirty, +2/-1, looks merged"
        );
        let mut r = test_row(true, false, None, None, None);
        r.branch_matches = false;
        assert_eq!(state_text(&r), "wrong-branch");
        r.branch_matches = true;
        r.registered = false;
        assert_eq!(state_text(&r), "unregistered");
    }

    #[test]
    fn base_lag_is_named_and_yellow_but_zero_or_unknown_is_silent() {
        let mut r = test_row(true, false, None, None, None);
        r.base = Some("feature/canonical".into());
        r.base_behind = Some(0);
        assert_eq!(state_text(&r), "ok");
        assert_eq!(state_colour(&r), StateColour::Green);
        r.base_behind = None;
        assert_eq!(state_text(&r), "ok", "unknown lag is not a problem");
        r.base_behind = Some(1);
        assert_eq!(state_text(&r), "behind base feature/canonical by 1");
        assert_eq!(state_colour(&r), StateColour::Yellow);
        r.dirty = true;
        r.ahead = Some(2);
        r.behind = Some(0);
        assert_eq!(
            state_text(&r),
            "dirty, +2/-0, behind base feature/canonical by 1"
        );
        r.exists = false;
        assert_eq!(state_colour(&r), StateColour::Red, "structure still wins");
    }

    #[test]
    fn state_colour_is_red_for_structure_yellow_for_work_green_otherwise() {
        assert_eq!(
            state_colour(&test_row(true, false, Some(0), Some(0), None)),
            StateColour::Green
        );
        assert_eq!(
            state_colour(&test_row(false, false, None, None, None)),
            StateColour::Red
        );
        assert_eq!(
            state_colour(&test_row(true, true, None, None, None)),
            StateColour::Yellow
        );
        assert_eq!(
            state_colour(&test_row(true, false, Some(1), Some(0), None)),
            StateColour::Yellow
        );
        assert_eq!(
            state_colour(&test_row(true, false, None, None, Some("hint"))),
            StateColour::Yellow
        );
        let mut r = test_row(true, true, None, None, None);
        r.registered = false;
        assert_eq!(
            state_colour(&r),
            StateColour::Red,
            "structure beats dirtiness"
        );
    }
}
