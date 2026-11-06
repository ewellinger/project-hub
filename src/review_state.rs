//! The state of a review on its forge: status, approval, pipeline, threads
//! and conflicts, read through `glab` or `gh`.

use crate::error::Result;
use crate::review::{Provider, ReviewUrl, api_get, api_graphql};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Approval {
    Yes,
    Changes,
    No,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pipeline {
    Passed,
    Failed,
    Running,
    None,
}

/// What one forge read yields. `None` means the forge did not say (`?`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Fields {
    pub status: Option<RequestStatus>,
    pub approved: Option<Approval>,
    pub pipeline: Option<Pipeline>,
    /// Unresolved threads on the first page of results.
    pub threads: Option<u64>,
    /// The forge has more threads than that page held.
    #[serde(default)]
    pub threads_more: bool,
    pub conflicts: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewState {
    /// `Change.review_url` exactly as recorded when this was read.
    pub url: String,
    /// Seconds since the Unix epoch.
    pub read_at: u64,
    #[serde(flatten)]
    pub fields: Fields,
}

/// One merge request, its `/approvals` and its first page of `/discussions`.
pub fn parse_gitlab_state(mr: &Value, approvals: &Value, discussions: &Value) -> Fields {
    let draft = mr["draft"].as_bool().or(mr["work_in_progress"].as_bool()) == Some(true);
    let status = match mr["state"].as_str() {
        Some("opened" | "locked") if draft => Some(RequestStatus::Draft),
        Some("opened" | "locked") => Some(RequestStatus::Open),
        Some("merged") => Some(RequestStatus::Merged),
        Some("closed") => Some(RequestStatus::Closed),
        _ => None,
    };
    let approved = if mr["detailed_merge_status"] == "requested_changes" {
        Some(Approval::Changes)
    } else {
        // `approved` is true with no approver when the project requires none, so ask for one.
        let by_someone = approvals["approved_by"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        approvals["approved"].as_bool().map(|a| {
            if a && by_someone {
                Approval::Yes
            } else {
                Approval::No
            }
        })
    };
    let pipeline = mr.get("head_pipeline").and_then(|p| {
        if p.is_null() {
            return Some(Pipeline::None);
        }
        Some(match p["status"].as_str()? {
            "skipped" => Pipeline::None,
            "success" => Pipeline::Passed,
            "failed" | "canceled" => Pipeline::Failed,
            _ => Pipeline::Running,
        })
    });
    let threads = discussions.as_array().map(|list| {
        list.iter()
            .filter(|d| {
                d["notes"].as_array().is_some_and(|notes| {
                    notes
                        .iter()
                        .any(|n| n["resolvable"] == true && n["resolved"] == false)
                })
            })
            .count() as u64
    });
    let conflicts = match mr["merge_status"].as_str() {
        Some("unchecked" | "checking") => None,
        _ => mr["has_conflicts"].as_bool(),
    };
    Fields {
        status,
        approved,
        pipeline,
        threads,
        // The read asks for 100 per page; a full page may not be all of them.
        threads_more: discussions.as_array().is_some_and(|l| l.len() == 100),
        conflicts,
    }
}

/// `data.repository.pullRequest` from [`GITHUB_QUERY`].
pub fn parse_github_state(pull_request: &Value) -> Fields {
    let pr = pull_request;
    let status = match pr["state"].as_str() {
        Some("OPEN") if pr["isDraft"] == true => Some(RequestStatus::Draft),
        Some("OPEN") => Some(RequestStatus::Open),
        Some("MERGED") => Some(RequestStatus::Merged),
        Some("CLOSED") => Some(RequestStatus::Closed),
        _ => None,
    };
    let approved = match pr["reviewDecision"].as_str() {
        Some("APPROVED") => Some(Approval::Yes),
        Some("CHANGES_REQUESTED") => Some(Approval::Changes),
        Some("REVIEW_REQUIRED") => Some(Approval::No),
        Some(_) => None,
        None => pr["latestOpinionatedReviews"]["nodes"]
            .as_array()
            .map(|nodes| {
                let any = |s: &str| nodes.iter().any(|n| n["state"] == s);
                if any("CHANGES_REQUESTED") {
                    Approval::Changes
                } else if any("APPROVED") {
                    Approval::Yes
                } else {
                    Approval::No
                }
            }),
    };
    let pipeline = pr
        .pointer("/commits/nodes/0/commit/statusCheckRollup")
        .and_then(|rollup| {
            if rollup.is_null() {
                return Some(Pipeline::None);
            }
            match rollup["state"].as_str()? {
                "SUCCESS" => Some(Pipeline::Passed),
                "FAILURE" | "ERROR" => Some(Pipeline::Failed),
                "PENDING" | "EXPECTED" => Some(Pipeline::Running),
                _ => None,
            }
        });
    let nodes = pr["reviewThreads"]["nodes"].as_array();
    let threads = nodes.map(|n| n.iter().filter(|t| t["isResolved"] == false).count() as u64);
    let threads_more = match (nodes, pr["reviewThreads"]["totalCount"].as_u64()) {
        (Some(n), Some(total)) => total > n.len() as u64,
        _ => false,
    };
    let conflicts = match pr["mergeable"].as_str() {
        Some("CONFLICTING") => Some(true),
        Some("MERGEABLE") => Some(false),
        _ => None,
    };
    Fields {
        status,
        approved,
        pipeline,
        threads,
        threads_more,
        conflicts,
    }
}

const GITHUB_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){state isDraft mergeable reviewDecision latestOpinionatedReviews(first:20){nodes{state}} reviewThreads(first:100){totalCount nodes{isResolved}} commits(last:1){nodes{commit{statusCheckRollup{state}}}}}}}";

/// Read the review's state from its forge. On GitLab only the merge request
/// itself must answer; approvals and discussions that fail become unknown.
pub fn fetch_state(url: &ReviewUrl) -> Result<Fields> {
    match url.provider {
        Provider::GitLab => {
            let base = format!(
                "projects/{}/merge_requests/{}",
                url.path.replace('/', "%2F"),
                url.number
            );
            let get = |endpoint: &str| api_get("glab", &url.host, endpoint);
            let (mr, approvals, discussions) = std::thread::scope(|scope| {
                let approvals = scope.spawn(|| get(&format!("{base}/approvals")));
                let discussions = scope.spawn(|| get(&format!("{base}/discussions?per_page=100")));
                let mr = get(&base);
                (
                    mr,
                    approvals.join().ok().and_then(|r| r.ok()),
                    discussions.join().ok().and_then(|r| r.ok()),
                )
            });
            Ok(parse_gitlab_state(
                &mr?,
                &approvals.unwrap_or(Value::Null),
                &discussions.unwrap_or(Value::Null),
            ))
        }
        Provider::GitHub => {
            let (owner, name) = url.path.split_once('/').unwrap_or_default();
            let response = api_graphql(&url.host, GITHUB_QUERY, owner, name, url.number)?;
            Ok(parse_github_state(
                response
                    .pointer("/data/repository/pullRequest")
                    .unwrap_or(&Value::Null),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn gitlab_open_request_with_everything_reported() {
        let mr = json!({"state":"opened","draft":false,"has_conflicts":true,"merge_status":"cannot_be_merged",
                        "head_pipeline":{"status":"failed"}});
        let approvals = json!({"approved":false});
        let discussions = json!([
            {"notes":[{"resolvable":true,"resolved":false}]},
            {"notes":[{"resolvable":true,"resolved":true}]},
            {"notes":[{"resolvable":false}]}]);
        assert_eq!(
            parse_gitlab_state(&mr, &approvals, &discussions),
            Fields {
                status: Some(RequestStatus::Open),
                approved: Some(Approval::No),
                pipeline: Some(Pipeline::Failed),
                threads: Some(1),
                threads_more: false,
                conflicts: Some(true)
            }
        );
    }

    fn gitlab_status(mr: Value) -> Option<RequestStatus> {
        parse_gitlab_state(&mr, &Value::Null, &Value::Null).status
    }

    #[test]
    fn gitlab_draft_merged_and_closed_states() {
        assert_eq!(
            gitlab_status(json!({"state":"opened","draft":true})),
            Some(RequestStatus::Draft)
        );
        assert_eq!(
            gitlab_status(json!({"state":"opened","work_in_progress":true})),
            Some(RequestStatus::Draft)
        );
        assert_eq!(
            gitlab_status(json!({"state":"locked"})),
            Some(RequestStatus::Open)
        );
        assert_eq!(
            gitlab_status(json!({"state":"merged"})),
            Some(RequestStatus::Merged)
        );
        assert_eq!(
            gitlab_status(json!({"state":"closed"})),
            Some(RequestStatus::Closed)
        );
        assert_eq!(gitlab_status(json!({"state":"weird"})), None);
    }

    fn gitlab_pipeline(mr: Value) -> Option<Pipeline> {
        parse_gitlab_state(&mr, &Value::Null, &Value::Null).pipeline
    }

    #[test]
    fn gitlab_pipeline_null_is_none_and_absent_is_unknown() {
        assert_eq!(
            gitlab_pipeline(json!({"head_pipeline":null})),
            Some(Pipeline::None)
        );
        assert_eq!(gitlab_pipeline(json!({})), None);
        for (status, want) in [
            ("skipped", Pipeline::None),
            ("success", Pipeline::Passed),
            ("failed", Pipeline::Failed),
            ("canceled", Pipeline::Failed),
            ("pending", Pipeline::Running),
            ("manual", Pipeline::Running),
        ] {
            assert_eq!(
                gitlab_pipeline(json!({"head_pipeline":{"status":status}})),
                Some(want),
                "{status}"
            );
        }
    }

    #[test]
    fn gitlab_unchecked_merge_status_leaves_conflicts_unknown() {
        for status in ["checking", "unchecked"] {
            let mr = json!({"has_conflicts":false,"merge_status":status});
            assert_eq!(
                parse_gitlab_state(&mr, &Value::Null, &Value::Null).conflicts,
                None,
                "{status}"
            );
        }
    }

    #[test]
    fn gitlab_requested_changes_wins_over_the_approved_flag() {
        let mr = json!({"detailed_merge_status":"requested_changes"});
        let approvals = json!({"approved":true,"approved_by":[{"user":{"username":"ann"}}]});
        assert_eq!(
            parse_gitlab_state(&mr, &approvals, &Value::Null).approved,
            Some(Approval::Changes)
        );
        assert_eq!(
            parse_gitlab_state(&json!({}), &approvals, &Value::Null).approved,
            Some(Approval::Yes)
        );
    }

    #[test]
    fn gitlab_approved_with_no_approver_is_not_approved() {
        // Captured from a project that requires no approvals: nobody approved, yet `approved` is true.
        let approvals =
            json!({"approved":true,"approvals_required":0,"approvals_left":0,"approved_by":[]});
        assert_eq!(
            parse_gitlab_state(&json!({}), &approvals, &Value::Null).approved,
            Some(Approval::No)
        );
    }

    #[test]
    fn gitlab_null_side_responses_degrade_to_unknown() {
        let f = parse_gitlab_state(&json!({"state":"opened"}), &Value::Null, &Value::Null);
        assert_eq!((f.approved, f.threads), (None, None));
    }

    #[test]
    fn gitlab_a_full_page_of_discussions_sets_threads_more() {
        let open = json!({"notes":[{"resolvable":true,"resolved":false}]});
        let full = Value::Array(vec![open.clone(); 100]);
        let f = parse_gitlab_state(&json!({}), &Value::Null, &full);
        assert_eq!((f.threads, f.threads_more), (Some(100), true));
        let short = Value::Array(vec![open; 99]);
        let f = parse_gitlab_state(&json!({}), &Value::Null, &short);
        assert_eq!((f.threads, f.threads_more), (Some(99), false));
    }

    #[test]
    fn github_open_request_with_everything_reported() {
        let pr = json!({"state":"OPEN","isDraft":false,"mergeable":"MERGEABLE","reviewDecision":"APPROVED",
            "latestOpinionatedReviews":{"nodes":[]},
            "reviewThreads":{"totalCount":2,"nodes":[{"isResolved":false},{"isResolved":true}]},
            "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}});
        assert_eq!(
            parse_github_state(&pr),
            Fields {
                status: Some(RequestStatus::Open),
                approved: Some(Approval::Yes),
                pipeline: Some(Pipeline::Passed),
                threads: Some(1),
                threads_more: false,
                conflicts: Some(false)
            }
        );
    }

    #[test]
    fn github_review_decision_maps_directly() {
        for (decision, want) in [
            ("APPROVED", Approval::Yes),
            ("CHANGES_REQUESTED", Approval::Changes),
            ("REVIEW_REQUIRED", Approval::No),
        ] {
            let pr = json!({"reviewDecision":decision,
                "latestOpinionatedReviews":{"nodes":[{"state":"APPROVED"}]}});
            assert_eq!(parse_github_state(&pr).approved, Some(want), "{decision}");
        }
    }

    #[test]
    fn github_null_review_decision_falls_back_to_the_latest_reviews() {
        let approved = |nodes: Value| {
            parse_github_state(&json!({"reviewDecision":null,
                "latestOpinionatedReviews":{"nodes":nodes}}))
            .approved
        };
        assert_eq!(approved(json!([{"state":"APPROVED"}])), Some(Approval::Yes));
        assert_eq!(
            approved(json!([{"state":"APPROVED"},{"state":"CHANGES_REQUESTED"}])),
            Some(Approval::Changes)
        );
        assert_eq!(approved(json!([])), Some(Approval::No));
    }

    #[test]
    fn github_null_rollup_is_no_pipeline_and_unknown_mergeable_is_unknown() {
        let pr = json!({"mergeable":"UNKNOWN",
            "commits":{"nodes":[{"commit":{"statusCheckRollup":null}}]}});
        let f = parse_github_state(&pr);
        assert_eq!((f.pipeline, f.conflicts), (Some(Pipeline::None), None));
        let conflicting = parse_github_state(&json!({"mergeable":"CONFLICTING"}));
        assert_eq!(conflicting.conflicts, Some(true));
        for (state, want) in [
            ("FAILURE", Pipeline::Failed),
            ("ERROR", Pipeline::Failed),
            ("PENDING", Pipeline::Running),
            ("EXPECTED", Pipeline::Running),
        ] {
            let pr =
                json!({"commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":state}}}]}});
            assert_eq!(parse_github_state(&pr).pipeline, Some(want), "{state}");
        }
    }

    #[test]
    fn github_more_threads_than_one_page_sets_threads_more() {
        let nodes = vec![json!({"isResolved":false}); 100];
        let pr = json!({"reviewThreads":{"totalCount":150,"nodes":nodes}});
        let f = parse_github_state(&pr);
        assert_eq!((f.threads, f.threads_more), (Some(100), true));
    }

    // Real responses, captured 2026-11-05. GitLab ones are trimmed to the keys the parser reads.
    #[test]
    fn captured_github_responses_parse() {
        let draft = json!({"state":"OPEN","isDraft":true,"mergeable":"MERGEABLE","reviewDecision":"REVIEW_REQUIRED",
            "latestOpinionatedReviews":{"nodes":[]},
            "reviewThreads":{"totalCount":3,"nodes":[{"isResolved":true},{"isResolved":true},{"isResolved":true}]},
            "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}});
        assert_eq!(
            parse_github_state(&draft),
            Fields {
                status: Some(RequestStatus::Draft),
                approved: Some(Approval::No),
                pipeline: Some(Pipeline::Passed),
                threads: Some(0),
                threads_more: false,
                conflicts: Some(false)
            }
        );
        let merged = json!({"state":"MERGED","isDraft":false,"mergeable":"UNKNOWN","reviewDecision":null,
            "latestOpinionatedReviews":{"nodes":[]},"reviewThreads":{"totalCount":0,"nodes":[]},
            "commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS"}}}]}});
        assert_eq!(
            parse_github_state(&merged),
            Fields {
                status: Some(RequestStatus::Merged),
                approved: Some(Approval::No),
                pipeline: Some(Pipeline::Passed),
                threads: Some(0),
                threads_more: false,
                conflicts: None
            }
        );
    }

    #[test]
    fn captured_gitlab_request_and_approvals_parse() {
        let mr = json!({"state":"opened","draft":false,"work_in_progress":false,"has_conflicts":false,
            "merge_status":"can_be_merged","detailed_merge_status":"discussions_not_resolved",
            "head_pipeline":{"status":"running"}});
        let approvals =
            json!({"approved":false,"approvals_required":2,"approvals_left":2,"approved_by":[]});
        assert_eq!(
            parse_gitlab_state(&mr, &approvals, &Value::Null),
            Fields {
                status: Some(RequestStatus::Open),
                approved: Some(Approval::No),
                pipeline: Some(Pipeline::Running),
                threads: None,
                threads_more: false,
                conflicts: Some(false)
            }
        );
        // A self-hosted instance with no approval requirement sends nulls for the counts.
        let optional = json!({"approved":false,"approvals_required":null,"approvals_left":null,"approved_by":[]});
        assert_eq!(
            parse_gitlab_state(&mr, &optional, &Value::Null).approved,
            Some(Approval::No)
        );
    }

    #[test]
    fn an_empty_object_parses_to_all_unknown() {
        assert_eq!(parse_github_state(&json!({})), Fields::default());
        assert_eq!(
            parse_gitlab_state(&json!({}), &json!({}), &json!({})),
            Fields::default()
        );
    }

    #[test]
    fn review_state_round_trips_with_flat_lowercase_fields() {
        let state = ReviewState {
            url: "https://gitlab.com/g/p/-/merge_requests/1".into(),
            read_at: 1_762_300_800,
            fields: Fields {
                status: Some(RequestStatus::Open),
                approved: Some(Approval::No),
                pipeline: Some(Pipeline::Failed),
                threads: Some(1),
                threads_more: false,
                conflicts: Some(true),
            },
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(
            value,
            json!({"url":"https://gitlab.com/g/p/-/merge_requests/1","read_at":1_762_300_800u64,
                "status":"open","approved":"no","pipeline":"failed","threads":1,
                "threads_more":false,"conflicts":true})
        );
        assert_eq!(serde_json::from_value::<ReviewState>(value).unwrap(), state);
        // `threads_more` is optional on read; `pipeline: "none"` is not unknown.
        let read: ReviewState = serde_json::from_value(json!({"url":"u","read_at":0,
            "status":null,"approved":null,"pipeline":"none","threads":null,"conflicts":null}))
        .unwrap();
        assert_eq!(read.fields.pipeline, Some(Pipeline::None));
        assert!(!read.fields.threads_more);
    }
}
