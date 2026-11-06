//! The last review state read from the forge for each role in review,
//! remembered in `<state_dir>/reviews/<feature>.json`.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use comfy_table::Cell;

use crate::error::{HubError, Result};
use crate::feature::{Feature, Stage};
use crate::hub::Hub;
use crate::manifest::write_json;
use crate::ops::status::{StateColour, StatusReport, StatusRow, comfy_colour, parallel_map};
use crate::ops::{format_age, styled_table};
use crate::review::{Provider, ReviewUrl, parse_review_url};
use crate::review_state::{Approval, Fields, Pipeline, RequestStatus, ReviewState, fetch_state};

pub type States = BTreeMap<String /* role */, ReviewState>;

pub struct Target {
    pub role: String,
    pub recorded: String,
    pub url: ReviewUrl,
}

/// Open changes of an open feature in stage `review` whose URL parses.
pub fn targets(feature: &Feature) -> Vec<Target> {
    if !feature.is_open() {
        return Vec::new();
    }
    feature
        .open_changes()
        .into_iter()
        .filter(|c| c.stage == Stage::Review)
        .filter_map(|c| {
            let recorded = c.review_url.clone()?;
            let url = parse_review_url(&recorded).ok()?;
            Some(Target {
                role: c.role.clone(),
                recorded,
                url,
            })
        })
        .collect()
}

/// Stored states for the feature's current targets whose stored `url`
/// equals the recorded one. Missing or unparseable file: empty.
pub fn remembered(hub: &Hub, feature: &Feature) -> States {
    let mut stored: States = std::fs::read_to_string(hub.reviews_path(&feature.name))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    targets(feature)
        .into_iter()
        .filter_map(|t| {
            let state = stored.remove(&t.role).filter(|s| s.url == t.recorded)?;
            Some((t.role, state))
        })
        .collect()
}

/// Read every target from the forge concurrently, keep the remembered state
/// of any that fail, store the result, and return it with warnings.
pub fn refresh(hub: &Hub, feature: &Feature) -> (States, Vec<String>) {
    let targets = targets(feature);
    if targets.is_empty() {
        return (States::new(), Vec::new());
    }
    let mut states = remembered(hub, feature);
    let mut warnings = Vec::new();
    let read_at = now_secs();
    let results = parallel_map(&targets, |t| fetch_state(&t.url));
    for (target, result) in targets.iter().zip(results) {
        match result {
            Ok(fields) => {
                states.insert(
                    target.role.clone(),
                    ReviewState {
                        url: target.recorded.clone(),
                        read_at,
                        fields,
                    },
                );
            }
            // The program is not on PATH; the message already says what to do.
            Err(HubError::Usage(message)) => warnings.push(message),
            Err(err) => warnings.push(format!(
                "review state not read for {}: {}",
                target.role,
                err.to_string().lines().next().unwrap_or_default()
            )),
        }
    }
    let path = hub.reviews_path(&feature.name);
    let dir = path.parent().expect("reviews path has a parent");
    let saved = std::fs::create_dir_all(dir)
        .map_err(|e| HubError::io(format!("creating {}", dir.display()), e))
        .and_then(|()| write_json(&path, &states));
    if let Err(err) = saved {
        warnings.push(format!("review state not saved: {err}"));
    }
    warnings.sort();
    warnings.dedup();
    (states, warnings)
}

#[derive(Debug)]
pub struct RefreshAll {
    pub features: usize,
    pub warnings: Vec<String>,
}

/// `refresh` for every open feature that has a target, concurrently.
pub fn refresh_all(hub: &Hub) -> Result<RefreshAll> {
    let features: Vec<Feature> = hub
        .list_features()?
        .into_iter()
        .filter(|f| !targets(f).is_empty())
        .collect();
    let mut warnings = parallel_map(&features, |f| refresh(hub, f).1).concat();
    warnings.sort();
    warnings.dedup();
    Ok(RefreshAll {
        features: features.len(),
        warnings,
    })
}

pub enum Source {
    Forge,
    Remembered,
}

/// Fill `row.review` for each target's row (same role, stage "review"),
/// replace its hint when the request is merged or closed, append warnings.
pub fn attach(hub: &Hub, feature: &Feature, report: &mut StatusReport, source: Source) {
    let (states, warnings) = match source {
        Source::Forge => refresh(hub, feature),
        Source::Remembered => (remembered(hub, feature), Vec::new()),
    };
    for target in targets(feature) {
        let Some(row) = report
            .rows
            .iter_mut()
            .find(|r| r.role == target.role && r.stage == "review")
        else {
            continue;
        };
        row.review = states.get(&target.role).cloned();
        let forge = target.url.provider.name();
        match row.review.as_ref().and_then(|s| s.fields.status) {
            Some(RequestStatus::Merged) => {
                row.hint = Some(format!(
                    "merged on {forge}; run hub feature merged {}",
                    target.role
                ))
            }
            Some(RequestStatus::Closed) => {
                row.hint = Some(format!("closed on {forge} without merging"))
            }
            _ => {}
        }
    }
    report.warnings.extend(warnings);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewWord {
    Merged,
    Closed,
    Conflicts,
    CiFailed,
    Threads(u64),
    CiRunning,
    Approved,
    Draft,
}

impl std::fmt::Display for ReviewWord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewWord::Merged => f.write_str("merged"),
            ReviewWord::Closed => f.write_str("closed"),
            ReviewWord::Conflicts => f.write_str("conflicts"),
            ReviewWord::CiFailed => f.write_str("ci failed"),
            ReviewWord::Threads(1) => f.write_str("1 thread"),
            ReviewWord::Threads(n) => write!(f, "{n} threads"),
            ReviewWord::CiRunning => f.write_str("ci running"),
            ReviewWord::Approved => f.write_str("approved"),
            ReviewWord::Draft => f.write_str("draft"),
        }
    }
}

impl serde::Serialize for ReviewWord {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl ReviewWord {
    pub fn colour(&self) -> Option<StateColour> {
        match self {
            ReviewWord::Merged | ReviewWord::Approved => Some(StateColour::Green),
            ReviewWord::Closed | ReviewWord::Conflicts | ReviewWord::CiFailed => {
                Some(StateColour::Red)
            }
            ReviewWord::Threads(_) | ReviewWord::CiRunning => Some(StateColour::Yellow),
            ReviewWord::Draft => None,
        }
    }
}

/// The first word that applies, in the Global Constraints order.
pub fn word(fields: &Fields) -> Option<ReviewWord> {
    match fields.status {
        Some(RequestStatus::Merged) => return Some(ReviewWord::Merged),
        Some(RequestStatus::Closed) => return Some(ReviewWord::Closed),
        _ => {}
    }
    if fields.conflicts == Some(true) {
        return Some(ReviewWord::Conflicts);
    }
    if fields.pipeline == Some(Pipeline::Failed) {
        return Some(ReviewWord::CiFailed);
    }
    if let Some(n @ 1..) = fields.threads {
        return Some(ReviewWord::Threads(n));
    }
    if fields.pipeline == Some(Pipeline::Running) {
        return Some(ReviewWord::CiRunning);
    }
    if fields.approved == Some(Approval::Yes) {
        return Some(ReviewWord::Approved);
    }
    (fields.status == Some(RequestStatus::Draft)).then_some(ReviewWord::Draft)
}

pub const REVIEW_HEADERS: [&str; 7] = [
    "REVIEW",
    "REQUEST",
    "STATUS",
    "APPROVED",
    "PIPELINE",
    "THREADS",
    "CONFLICTS",
];

/// `None` unless the row is in stage "review" with a parseable URL.
pub fn review_cells(row: &StatusRow) -> Option<[(String, Option<StateColour>); 7]> {
    if row.stage != "review" {
        return None;
    }
    let url = parse_review_url(row.review_url.as_deref()?).ok()?;
    let request = match url.provider {
        Provider::GitLab => format!("!{}", url.number),
        Provider::GitHub => format!("#{}", url.number),
    };
    let plain = |text: &str| (text.to_string(), None);
    let unknown = || plain("?");
    let Some(f) = row.review.as_ref().map(|s| &s.fields) else {
        return Some([
            plain(&row.role),
            plain(&request),
            unknown(),
            unknown(),
            unknown(),
            unknown(),
            unknown(),
        ]);
    };
    let finished = matches!(
        f.status,
        Some(RequestStatus::Merged | RequestStatus::Closed)
    );
    let status = match f.status {
        None => unknown(),
        Some(RequestStatus::Open) => plain("open"),
        Some(RequestStatus::Draft) => plain("draft"),
        Some(RequestStatus::Merged) => ("merged".into(), Some(StateColour::Green)),
        Some(RequestStatus::Closed) => ("closed".into(), Some(StateColour::Red)),
    };
    let pipeline = match f.pipeline {
        None => unknown(),
        Some(Pipeline::Passed) => ("passed".into(), Some(StateColour::Green)),
        Some(Pipeline::Failed) => ("failed".into(), Some(StateColour::Red)),
        Some(Pipeline::Running) => ("running".into(), Some(StateColour::Yellow)),
        Some(Pipeline::None) => plain("none"),
    };
    let [approved, threads, conflicts] = if finished {
        [plain("-"), plain("-"), plain("-")]
    } else {
        [
            match f.approved {
                None => unknown(),
                Some(Approval::Yes) => ("yes".into(), Some(StateColour::Green)),
                Some(Approval::Changes) => ("changes".into(), Some(StateColour::Yellow)),
                Some(Approval::No) => plain("no"),
            },
            match f.threads {
                None => unknown(),
                Some(n) => (
                    format!("{n}{} open", if f.threads_more { "+" } else { "" }),
                    (n > 0).then_some(StateColour::Yellow),
                ),
            },
            match f.conflicts {
                None => unknown(),
                Some(true) => ("yes".into(), Some(StateColour::Red)),
                Some(false) => plain("no"),
            },
        ]
    };
    Some([
        plain(&row.role),
        plain(&request),
        status,
        approved,
        pipeline,
        threads,
        conflicts,
    ])
}

/// "review state read 4m ago", from the oldest `read_at`; `None` when no row has state.
/// The age is `now.saturating_sub(read_at)`, so a clock that stepped back reads as 0s.
pub fn review_age_line(rows: &[&StatusRow], now: u64) -> Option<String> {
    let oldest = rows
        .iter()
        .filter_map(|r| r.review.as_ref().map(|s| s.read_at))
        .min()?;
    let age = format_age(Duration::from_secs(now.saturating_sub(oldest)));
    Some(format!("review state read {age} ago"))
}

/// Blank line, the table, blank line, the age line; empty when no row has cells.
pub fn section_lines(rows: &[&StatusRow], now: u64) -> Vec<String> {
    let table: Vec<Vec<Cell>> = rows
        .iter()
        .filter_map(|r| review_cells(r))
        .map(|cells| {
            cells
                .into_iter()
                .map(|(text, colour)| match colour {
                    Some(c) => Cell::new(text).fg(comfy_colour(c)),
                    None => Cell::new(text),
                })
                .collect()
        })
        .collect();
    if table.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![String::new()];
    lines.extend(styled_table(&REVIEW_HEADERS, table));
    if let Some(age) = review_age_line(rows, now) {
        lines.push(String::new());
        lines.push(age);
    }
    lines
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::status::StateColour::{Green, Red, Yellow};
    use crate::ops::status::tests::row;
    use crate::review_state::{Approval, Fields, Pipeline, RequestStatus};

    const MR: &str = "https://gitlab.example.com/g/proj/-/merge_requests/412";
    const PR: &str = "https://github.example.com/o/r/pull/87";

    fn review_row(role: &str, url: &str, fields: Option<Fields>) -> StatusRow {
        let mut r = row();
        r.role = role.into();
        r.stage = "review".into();
        r.review_url = Some(url.into());
        r.review = fields.map(|fields| ReviewState {
            url: url.into(),
            read_at: 1000,
            fields,
        });
        r
    }

    fn texts(r: &StatusRow) -> Vec<String> {
        review_cells(r).unwrap().into_iter().map(|c| c.0).collect()
    }

    fn colours(r: &StatusRow) -> Vec<Option<StateColour>> {
        review_cells(r).unwrap().into_iter().map(|c| c.1).collect()
    }

    fn open_fields() -> Fields {
        Fields {
            status: Some(RequestStatus::Open),
            approved: Some(Approval::No),
            pipeline: Some(Pipeline::Failed),
            threads: Some(2),
            threads_more: false,
            conflicts: Some(false),
        }
    }

    #[test]
    fn the_word_is_the_first_that_applies() {
        let open = Fields {
            status: Some(RequestStatus::Open),
            approved: Some(Approval::No),
            pipeline: Some(Pipeline::Passed),
            threads: Some(0),
            threads_more: false,
            conflicts: Some(false),
        };
        let merged = Fields {
            status: Some(RequestStatus::Merged),
            pipeline: Some(Pipeline::Failed),
            threads: Some(2),
            conflicts: Some(true),
            ..open.clone()
        };
        assert_eq!(word(&merged), Some(ReviewWord::Merged));
        let closed = Fields {
            status: Some(RequestStatus::Closed),
            ..merged.clone()
        };
        assert_eq!(word(&closed), Some(ReviewWord::Closed));
        let conflicts = Fields {
            conflicts: Some(true),
            pipeline: Some(Pipeline::Failed),
            ..open.clone()
        };
        assert_eq!(word(&conflicts), Some(ReviewWord::Conflicts));
        let failed = Fields {
            pipeline: Some(Pipeline::Failed),
            threads: Some(2),
            ..open.clone()
        };
        assert_eq!(word(&failed), Some(ReviewWord::CiFailed));
        let threads = Fields {
            threads: Some(2),
            pipeline: Some(Pipeline::Running),
            ..open.clone()
        };
        assert_eq!(word(&threads), Some(ReviewWord::Threads(2)));
        let running = Fields {
            pipeline: Some(Pipeline::Running),
            approved: Some(Approval::Yes),
            ..open.clone()
        };
        assert_eq!(word(&running), Some(ReviewWord::CiRunning));
        let approved = Fields {
            approved: Some(Approval::Yes),
            ..open.clone()
        };
        assert_eq!(word(&approved), Some(ReviewWord::Approved));
        let draft = Fields {
            status: Some(RequestStatus::Draft),
            ..Fields::default()
        };
        assert_eq!(word(&draft), Some(ReviewWord::Draft));
        assert_eq!(word(&open), None);
        assert_eq!(word(&Fields::default()), None);
    }

    #[test]
    fn words_display_and_serialise_as_text() {
        assert_eq!(ReviewWord::Threads(2).to_string(), "2 threads");
        assert_eq!(ReviewWord::Threads(1).to_string(), "1 thread");
        assert_eq!(ReviewWord::CiRunning.to_string(), "ci running");
        assert_eq!(
            serde_json::to_string(&ReviewWord::CiFailed).unwrap(),
            "\"ci failed\""
        );
    }

    #[test]
    fn cells_for_an_open_gitlab_request() {
        let r = review_row("api", MR, Some(open_fields()));
        assert_eq!(
            texts(&r),
            ["api", "!412", "open", "no", "failed", "2 open", "no"]
        );
        assert_eq!(
            colours(&r),
            [None, None, None, None, Some(Red), Some(Yellow), None]
        );
    }

    #[test]
    fn cells_for_a_merged_github_request_dash_out_what_no_longer_applies() {
        let fields = Fields {
            status: Some(RequestStatus::Merged),
            pipeline: Some(Pipeline::Passed),
            ..open_fields()
        };
        let r = review_row("bff", PR, Some(fields));
        assert_eq!(texts(&r), ["bff", "#87", "merged", "-", "passed", "-", "-"]);
        assert_eq!(
            colours(&r),
            [None, None, Some(Green), None, Some(Green), None, None]
        );
    }

    #[test]
    fn cells_without_state_are_question_marks() {
        let r = review_row("api", MR, None);
        assert_eq!(texts(&r), ["api", "!412", "?", "?", "?", "?", "?"]);
        assert_eq!(colours(&r), [None; 7]);
    }

    #[test]
    fn threads_more_renders_a_plus() {
        let fields = Fields {
            threads: Some(100),
            threads_more: true,
            ..open_fields()
        };
        assert_eq!(texts(&review_row("api", MR, Some(fields)))[5], "100+ open");
    }

    #[test]
    fn a_working_row_or_an_unparseable_url_has_no_cells() {
        let mut working = review_row("api", MR, Some(open_fields()));
        working.stage = "working".into();
        assert!(review_cells(&working).is_none());
        let bad = review_row("api", "https://gitlab.example.com/not/a/request", None);
        assert!(review_cells(&bad).is_none());
    }

    #[test]
    fn the_age_line_uses_the_oldest_read() {
        let mut newer = review_row("bff", PR, Some(open_fields()));
        newer.review.as_mut().unwrap().read_at = 1180;
        let older = review_row("api", MR, Some(open_fields()));
        let none = review_row("web", MR, None);
        assert_eq!(
            review_age_line(&[&newer, &none, &older], 1240).as_deref(),
            Some("review state read 4m ago")
        );
        assert_eq!(review_age_line(&[&none], 1240), None);
        assert_eq!(
            review_age_line(&[&older], 10).as_deref(),
            Some("review state read 0s ago")
        );
    }

    #[test]
    fn section_lines_are_empty_without_a_review_row() {
        let working = row();
        assert!(section_lines(&[&working], 1240).is_empty());
        let lines = section_lines(
            &[&working, &review_row("api", MR, Some(open_fields()))],
            1240,
        );
        assert_eq!(lines[0], "");
        assert!(lines[1].starts_with("REVIEW"), "{lines:?}");
        assert_eq!(lines[lines.len() - 2], "");
        assert_eq!(lines[lines.len() - 1], "review state read 4m ago");
    }
}
