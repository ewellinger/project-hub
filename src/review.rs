//! Review URLs: parsing, matching them to manifest roles, and reading the
//! merge/pull request from the provider through `glab` or `gh`.

use crate::error::{HubError, Result};
use crate::manifest::{Manifest, RepoSpec};
use crate::names;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    GitLab,
    GitHub,
}

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Provider::GitLab => "GitLab",
            Provider::GitHub => "GitHub",
        }
    }
}

/// Host, repository path, and review number as the URL names them. `host`
/// is lowercase and keeps an explicit port. A GitLab `path` is lowercased,
/// the canonical form of a project path; a GitHub `path` keeps the URL's
/// case, which is how GitHub displays an owner and repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewUrl {
    pub provider: Provider,
    pub host: String,
    pub path: String,
    pub number: u64,
}

impl ReviewUrl {
    pub fn canonical(&self) -> String {
        match self.provider {
            Provider::GitLab => format!(
                "https://{}/{}/-/merge_requests/{}",
                self.host, self.path, self.number
            ),
            Provider::GitHub => format!("https://{}/{}/pull/{}", self.host, self.path, self.number),
        }
    }

    pub fn identity(&self) -> RemoteIdentity {
        RemoteIdentity {
            host: self.host.clone(),
            path: self.path.clone(),
        }
    }
}

/// A repository as a host plus namespace path, the form both a review URL
/// and a git remote reduce to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIdentity {
    pub host: String,
    pub path: String,
}

impl RemoteIdentity {
    /// Hosts (with port) must be identical; paths compare without case, as
    /// neither provider allows two repositories that differ only by case.
    pub fn matches(&self, other: &RemoteIdentity) -> bool {
        self.host == other.host && self.path.eq_ignore_ascii_case(&other.path)
    }
}

fn is_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'
}

/// One namespace or repository segment: non-empty, `[A-Za-z0-9._-]`, not
/// `.`/`..`, no `.git` suffix.
fn valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.ends_with(".git")
        && segment.chars().all(is_segment_char)
}

/// Lowercase host with an optional `:port`, rejecting user info.
fn parse_authority(authority: &str) -> Option<String> {
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return None;
    }
    if let Some(p) = port
        && (p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some(authority.to_ascii_lowercase())
}

fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|n| *n > 0)
}

/// Accepts exactly the two documented layouts over HTTPS; see the spec.
pub fn parse_review_url(input: &str) -> Result<ReviewUrl> {
    let bad = |why: &str| {
        HubError::Usage(format!(
            "unsupported review URL '{input}': {why}; expected \
             https://HOST/GROUP/PROJECT/-/merge_requests/N or https://HOST/OWNER/REPO/pull/N"
        ))
    };
    let rest = input
        .strip_prefix("https://")
        .ok_or_else(|| bad("only https:// URLs are supported"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = parse_authority(authority).ok_or_else(|| {
        bad("the host must be a plain hostname with an optional port and no credentials")
    })?;
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return Err(bad("no repository path"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if let Some(dash) = segments.iter().position(|s| *s == "-") {
        let repo = &segments[..dash];
        let tail = &segments[dash + 1..];
        if repo.len() < 2 || !repo.iter().all(|s| valid_segment(s)) {
            return Err(bad(
                "a GitLab path needs at least a namespace and a project",
            ));
        }
        if tail.len() != 2 || tail[0] != "merge_requests" {
            return Err(bad("expected /-/merge_requests/N and nothing after it"));
        }
        let number = parse_number(tail[1])
            .ok_or_else(|| bad("the merge request number must be a positive integer"))?;
        return Ok(ReviewUrl {
            provider: Provider::GitLab,
            host,
            path: repo.join("/").to_ascii_lowercase(),
            number,
        });
    }
    if segments.len() == 4 && segments[2] == "pull" {
        if !valid_segment(segments[0]) || !valid_segment(segments[1]) {
            return Err(bad("a GitHub path is OWNER/REPOSITORY"));
        }
        let number = parse_number(segments[3])
            .ok_or_else(|| bad("the pull request number must be a positive integer"))?;
        return Ok(ReviewUrl {
            provider: Provider::GitHub,
            host,
            path: format!("{}/{}", segments[0], segments[1]),
            number,
        });
    }
    Err(bad("not a merge request or pull request path"))
}

/// Reduce a git remote to host and path. `None` for anything that is not a
/// hosted repository (a filesystem path, an empty string).
pub fn parse_remote(remote: &str) -> Option<RemoteIdentity> {
    let (authority, path) = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return None;
        }
        rest.split_once('/')?
    } else {
        // SCP-style `[user@]host:path`; a leading `/` or `.` is a local path.
        if remote.starts_with('/') || remote.starts_with('.') {
            return None;
        }
        remote.split_once(':')?
    };
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if authority.is_empty() {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() || path.split('/').any(|s| s.is_empty()) {
        return None;
    }
    Some(RemoteIdentity {
        host: authority.to_ascii_lowercase(),
        path: path.to_string(),
    })
}

/// The one role whose manifest remote is the repository the URL names.
pub fn match_role<'a>(manifest: &'a Manifest, url: &ReviewUrl) -> Result<&'a RepoSpec> {
    let wanted = url.identity();
    let matches: Vec<&RepoSpec> = manifest
        .repos
        .iter()
        .filter(|r| parse_remote(&r.remote).is_some_and(|id| id.matches(&wanted)))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => {
            let known: Vec<String> = manifest
                .repos
                .iter()
                .map(|r| format!("{}={}", r.role, r.remote))
                .collect();
            Err(HubError::Precondition(format!(
                "no registered role has its origin at https://{}/{}; registered remotes: {}",
                wanted.host,
                wanted.path,
                if known.is_empty() {
                    "(none)".to_string()
                } else {
                    known.join(", ")
                }
            )))
        }
        many => {
            let roles: Vec<&str> = many.iter().map(|r| r.role.as_str()).collect();
            Err(HubError::Precondition(format!(
                "roles {} all point at https://{}/{}; the URL must resolve to exactly one role",
                roles.join(", "),
                wanted.host,
                wanted.path
            )))
        }
    }
}

/// What `start-review` needs from the provider, identical for both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewInfo {
    pub url: ReviewUrl,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: String,
}

fn field<'a>(
    value: &'a serde_json::Value,
    path: &[&str],
    what: &str,
) -> Result<&'a serde_json::Value> {
    let mut cur = value;
    for key in path {
        cur = cur.get(key).ok_or_else(|| {
            HubError::Precondition(format!("{what} response has no '{}' field", path.join(".")))
        })?;
    }
    Ok(cur)
}

fn str_field(value: &serde_json::Value, path: &[&str], what: &str) -> Result<String> {
    field(value, path, what)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| {
            HubError::Precondition(format!(
                "{what} response: '{}' is not a string",
                path.join(".")
            ))
        })
}

fn u64_field(value: &serde_json::Value, path: &[&str], what: &str) -> Result<u64> {
    field(value, path, what)?.as_u64().ok_or_else(|| {
        HubError::Precondition(format!(
            "{what} response: '{}' is not a number",
            path.join(".")
        ))
    })
}

fn bool_field(value: &serde_json::Value, path: &[&str], what: &str) -> Result<bool> {
    field(value, path, what)?.as_bool().ok_or_else(|| {
        HubError::Precondition(format!(
            "{what} response: '{}' is not a boolean",
            path.join(".")
        ))
    })
}

fn branch(value: &serde_json::Value, path: &[&str], what: &str) -> Result<String> {
    let name = str_field(value, path, what)?;
    names::validate_branch_name(&name).map_err(|e| {
        HubError::Precondition(format!("{what} response: '{}': {e}", path.join(".")))
    })?;
    Ok(name)
}

fn sha(value: &serde_json::Value, path: &[&str], what: &str) -> Result<String> {
    let sha = str_field(value, path, what)?;
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(HubError::Precondition(format!(
            "{what} response: '{}' is not a commit id",
            path.join(".")
        )));
    }
    Ok(sha.to_ascii_lowercase())
}

/// The provider's own URL for the review must name the repository and
/// number that were asked for; a redirect to another project is refused.
fn same_review(url: &ReviewUrl, reported: &str, what: &str) -> Result<()> {
    let parsed = parse_review_url(reported)
        .map_err(|e| HubError::Precondition(format!("{what} response: web URL: {e}")))?;
    if parsed.provider != url.provider
        || parsed.number != url.number
        || !parsed.identity().matches(&url.identity())
    {
        return Err(HubError::Precondition(format!(
            "{what} response names {reported}, not {}",
            url.canonical()
        )));
    }
    Ok(())
}

/// Validate a GitLab project and merge request response pair.
pub fn parse_gitlab(
    url: &ReviewUrl,
    project: &serde_json::Value,
    mr: &serde_json::Value,
) -> Result<ReviewInfo> {
    let project_id = u64_field(project, &["id"], "GitLab project")?;
    let path = str_field(project, &["path_with_namespace"], "GitLab project")?;
    if !path.eq_ignore_ascii_case(&url.path) {
        return Err(HubError::Precondition(format!(
            "GitLab project response is for {path}, not {}",
            url.path
        )));
    }
    let what = "GitLab merge request";
    if u64_field(mr, &["iid"], what)? != url.number {
        return Err(HubError::Precondition(format!(
            "{what} response is not !{}",
            url.number
        )));
    }
    let state = str_field(mr, &["state"], what)?;
    if state != "opened" {
        return Err(HubError::Precondition(format!(
            "merge request !{} is {state}; only an open merge request can be reviewed",
            url.number
        )));
    }
    let source_project = u64_field(mr, &["source_project_id"], what)?;
    let target_project = u64_field(mr, &["target_project_id"], what)?;
    if target_project != project_id || source_project != project_id {
        return Err(HubError::Precondition(format!(
            "merge request !{} comes from another project (fork); only same-repository reviews are supported",
            url.number
        )));
    }
    same_review(url, &str_field(mr, &["web_url"], what)?, what)?;
    Ok(ReviewInfo {
        url: url.clone(),
        source_branch: branch(mr, &["source_branch"], what)?,
        target_branch: branch(mr, &["target_branch"], what)?,
        head_sha: sha(mr, &["sha"], what)?,
    })
}

/// Validate a GitHub pull request response.
pub fn parse_github(url: &ReviewUrl, pr: &serde_json::Value) -> Result<ReviewInfo> {
    let what = "GitHub pull request";
    if u64_field(pr, &["number"], what)? != url.number {
        return Err(HubError::Precondition(format!(
            "{what} response is not #{}",
            url.number
        )));
    }
    let state = str_field(pr, &["state"], what)?;
    if state != "open" || bool_field(pr, &["merged"], what)? {
        return Err(HubError::Precondition(format!(
            "pull request #{} is {}; only an open pull request can be reviewed",
            url.number,
            if state == "open" {
                "merged"
            } else {
                state.as_str()
            }
        )));
    }
    if field(pr, &["head", "repo"], what)?.is_null() {
        return Err(HubError::Precondition(format!(
            "pull request #{}'s source repository no longer exists",
            url.number
        )));
    }
    let base_repo = str_field(pr, &["base", "repo", "full_name"], what)?;
    if !base_repo.eq_ignore_ascii_case(&url.path) {
        return Err(HubError::Precondition(format!(
            "{what} response targets {base_repo}, not {}",
            url.path
        )));
    }
    if u64_field(pr, &["head", "repo", "id"], what)?
        != u64_field(pr, &["base", "repo", "id"], what)?
    {
        return Err(HubError::Precondition(format!(
            "pull request #{} comes from a fork; only same-repository reviews are supported",
            url.number
        )));
    }
    same_review(url, &str_field(pr, &["html_url"], what)?, what)?;
    Ok(ReviewInfo {
        url: url.clone(),
        source_branch: branch(pr, &["head", "ref"], what)?,
        target_branch: branch(pr, &["base", "ref"], what)?,
        head_sha: sha(pr, &["head", "sha"], what)?,
    })
}

/// `<program> api --hostname HOST --method GET ENDPOINT`, parsed as JSON.
fn api_get(program: &str, host: &str, endpoint: &str) -> Result<serde_json::Value> {
    let out = Command::new(program)
        .args(["api", "--hostname", host, "--method", "GET", endpoint])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HubError::Usage(format!(
                    "{program} is not on PATH; install it and run `{program} auth login --hostname {host}`"
                ))
            } else {
                HubError::io(format!("running {program}"), e)
            }
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(HubError::Precondition(format!(
            "{program} api {endpoint} on {host} failed: {stderr}\n\
             check `{program} auth status --hostname {host}` and that the review exists and is readable"
        )));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| {
        HubError::Precondition(format!(
            "{program} api {endpoint} on {host} returned malformed JSON: {e}"
        ))
    })
}

/// Read the review through the installed provider CLI. Only the CLI for the
/// URL's provider is needed; hub stores no tokens.
pub fn fetch_review(url: &ReviewUrl) -> Result<ReviewInfo> {
    match url.provider {
        Provider::GitLab => {
            let encoded = url.path.replace('/', "%2F");
            let project = api_get("glab", &url.host, &format!("projects/{encoded}"))?;
            let mr = api_get(
                "glab",
                &url.host,
                &format!("projects/{encoded}/merge_requests/{}", url.number),
            )?;
            parse_gitlab(url, &project, &mr)
        }
        Provider::GitHub => {
            let pr = api_get(
                "gh",
                &url.host,
                &format!("repos/{}/pulls/{}", url.path, url.number),
            )?;
            parse_github(url, &pr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, RepoSpec};

    fn spec(role: &str, remote: &str) -> RepoSpec {
        RepoSpec {
            role: role.into(),
            clone: format!("{role}-clone"),
            remote: remote.into(),
            base: "main".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        }
    }

    #[test]
    fn parses_gitlab_and_github_urls_with_decorations() {
        let mr = parse_review_url(
            "https://GitLab.com/acme/platform/ai/acme-agent-platform/-/merge_requests/486/?tab=diffs#note_1",
        )
        .unwrap();
        assert_eq!(mr.provider, Provider::GitLab);
        assert_eq!(mr.host, "gitlab.com");
        assert_eq!(mr.path, "acme/platform/ai/acme-agent-platform");
        assert_eq!(mr.number, 486);
        assert_eq!(
            mr.canonical(),
            "https://gitlab.com/acme/platform/ai/acme-agent-platform/-/merge_requests/486"
        );
        let pr = parse_review_url("https://github.example.com:8443/Owner/Repo.js/pull/12").unwrap();
        assert_eq!(pr.provider, Provider::GitHub);
        assert_eq!(pr.host, "github.example.com:8443");
        assert_eq!(pr.path, "Owner/Repo.js");
        assert_eq!(pr.number, 12);
        assert_eq!(
            pr.canonical(),
            "https://github.example.com:8443/Owner/Repo.js/pull/12"
        );
    }

    #[test]
    fn rejects_malformed_urls() {
        for bad in [
            "http://gitlab.com/g/p/-/merge_requests/1",
            "https://user:pw@gitlab.com/g/p/-/merge_requests/1",
            "https://gitlab.com/p/-/merge_requests/1",
            "https://gitlab.com/g/p/-/merge_requests/0",
            "https://gitlab.com/g/p/-/merge_requests/abc",
            "https://gitlab.com/g/p/-/merge_requests/1/diffs",
            "https://gitlab.com/g/../p/-/merge_requests/1",
            "https://gitlab.com/g//p/-/merge_requests/1",
            "https://gitlab.com/g/p%2Fx/-/merge_requests/1",
            "https://gitlab.com/g/p.git/-/merge_requests/1",
            "https://github.com/o/r/pull/",
            "https://github.com/o/r/pulls/1",
            "https://github.com/o/r/x/pull/1",
            "https://github.com/o/r/pull/1/files",
            "https://github.com/o/r/issues/1",
            "https://gitlab.com/g/p",
            "",
            "gitlab.com/g/p/-/merge_requests/1",
        ] {
            let err = parse_review_url(bad).unwrap_err();
            assert!(matches!(err, HubError::Usage(_)), "{bad}: {err}");
        }
    }

    #[test]
    fn normalizes_remote_forms_to_one_identity() {
        let want = RemoteIdentity {
            host: "gitlab.com".into(),
            path: "acme/platform/ai/acme-agent-platform".into(),
        };
        for remote in [
            "git@gitlab.com:acme/platform/ai/acme-agent-platform.git",
            "git@GitLab.com:acme/platform/ai/acme-agent-platform",
            "ssh://git@gitlab.com/acme/platform/ai/acme-agent-platform.git",
            "https://gitlab.com/acme/platform/ai/acme-agent-platform.git",
            "https://oauth2:token@gitlab.com/acme/platform/ai/acme-agent-platform.git/",
            "http://gitlab.com/acme/platform/ai/acme-agent-platform",
        ] {
            assert_eq!(parse_remote(remote).unwrap(), want, "{remote}");
        }
        assert_eq!(
            parse_remote("ssh://git@gitlab.com:2222/g/p.git")
                .unwrap()
                .host,
            "gitlab.com:2222",
            "the port stays part of the identity"
        );
        assert_eq!(
            parse_remote("gitlab-work:g/p.git").unwrap(),
            RemoteIdentity {
                host: "gitlab-work".into(),
                path: "g/p".into()
            },
            "an ssh alias is its own host and will simply not match"
        );
        for bad in [
            "/srv/git/x.git",
            "../relative",
            "",
            "https://",
            "https://host",
        ] {
            assert_eq!(parse_remote(bad), None, "{bad}");
        }
        assert!(want.matches(&RemoteIdentity {
            host: "gitlab.com".into(),
            path: "Acme/Platform/AI/Acme-Agent-Platform".into()
        }));
        assert!(!want.matches(&RemoteIdentity {
            host: "gitlab.com:2222".into(),
            path: want.path.clone()
        }));
    }

    #[test]
    fn match_role_requires_exactly_one_role() {
        let url = parse_review_url("https://gitlab.com/g/sub/proj/-/merge_requests/3").unwrap();
        let mut m = Manifest::new("acme", None);
        m.repos.push(spec("other", "git@gitlab.com:g/other.git"));
        let err = match_role(&m, &url).unwrap_err();
        assert!(
            matches!(&err, HubError::Precondition(msg) if msg.contains("no registered role") && msg.contains("other")),
            "{err}"
        );

        m.repos.push(spec("api", "git@gitlab.com:g/sub/proj.git"));
        assert_eq!(match_role(&m, &url).unwrap().role, "api");

        m.repos
            .push(spec("dup", "https://gitlab.com/G/Sub/Proj.git"));
        let err = match_role(&m, &url).unwrap_err();
        assert!(
            matches!(&err, HubError::Precondition(msg) if msg.contains("api") && msg.contains("dup")),
            "{err}"
        );

        // Same basename in another namespace is a different repository.
        let mut m = Manifest::new("acme", None);
        m.repos
            .push(spec("api", "git@gitlab.com:elsewhere/proj.git"));
        assert!(match_role(&m, &url).is_err());
        // An ssh alias cannot be resolved: the error names the remote.
        let mut m = Manifest::new("acme", None);
        m.repos.push(spec("api", "gitlab-work:g/sub/proj.git"));
        let err = match_role(&m, &url).unwrap_err();
        assert!(
            matches!(&err, HubError::Precondition(msg) if msg.contains("gitlab-work:g/sub/proj.git")),
            "{err}"
        );
    }

    fn gitlab_project() -> serde_json::Value {
        serde_json::json!({"id": 77, "path_with_namespace": "g/sub/proj"})
    }

    fn gitlab_mr() -> serde_json::Value {
        serde_json::json!({
            "iid": 3, "state": "opened",
            "web_url": "https://gitlab.com/g/sub/proj/-/merge_requests/3",
            "source_project_id": 77, "target_project_id": 77,
            "source_branch": "feature/x", "target_branch": "develop",
            "sha": "0123456789abcdef0123456789abcdef01234567", "draft": true
        })
    }

    #[test]
    fn gitlab_responses_validate_and_normalize() {
        let url = parse_review_url("https://gitlab.com/G/Sub/Proj/-/merge_requests/3").unwrap();
        let info = parse_gitlab(&url, &gitlab_project(), &gitlab_mr()).unwrap();
        assert_eq!(info.source_branch, "feature/x");
        assert_eq!(info.target_branch, "develop");
        assert_eq!(info.head_sha, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(
            info.url, url,
            "drafts are fine and the URL is the requested one"
        );

        let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
            ("closed", gitlab_project(), {
                let mut m = gitlab_mr();
                m["state"] = "merged".into();
                m
            }),
            ("fork", gitlab_project(), {
                let mut m = gitlab_mr();
                m["source_project_id"] = 78.into();
                m
            }),
            (
                "wrong project",
                {
                    let mut p = gitlab_project();
                    p["id"] = 5.into();
                    p
                },
                gitlab_mr(),
            ),
            (
                "wrong path",
                {
                    let mut p = gitlab_project();
                    p["path_with_namespace"] = "g/other".into();
                    p
                },
                gitlab_mr(),
            ),
            ("wrong iid", gitlab_project(), {
                let mut m = gitlab_mr();
                m["iid"] = 4.into();
                m
            }),
            ("bad sha", gitlab_project(), {
                let mut m = gitlab_mr();
                m["sha"] = "xyz".into();
                m
            }),
            ("bad branch", gitlab_project(), {
                let mut m = gitlab_mr();
                m["source_branch"] = "a..b".into();
                m
            }),
            ("empty target", gitlab_project(), {
                let mut m = gitlab_mr();
                m["target_branch"] = "".into();
                m
            }),
            ("foreign web_url", gitlab_project(), {
                let mut m = gitlab_mr();
                m["web_url"] = "https://gitlab.com/g/other/-/merge_requests/3".into();
                m
            }),
            ("not an object", serde_json::json!([]), gitlab_mr()),
        ];
        for (name, project, mr) in cases {
            let err = parse_gitlab(&url, &project, &mr).unwrap_err();
            assert!(matches!(err, HubError::Precondition(_)), "{name}: {err}");
        }
    }

    fn github_pr() -> serde_json::Value {
        serde_json::json!({
            "number": 12, "state": "open", "merged": false, "draft": true,
            "html_url": "https://github.com/Owner/Repo/pull/12",
            "head": {"ref": "topic", "sha": "89abcdef0123456789abcdef0123456789abcdef",
                     "repo": {"id": 9, "full_name": "Owner/Repo"}},
            "base": {"ref": "main", "repo": {"id": 9, "full_name": "Owner/Repo"}},
            "merge_commit_sha": "ffffffffffffffffffffffffffffffffffffffff"
        })
    }

    #[test]
    fn github_responses_validate_and_normalize() {
        let url = parse_review_url("https://github.com/owner/repo/pull/12").unwrap();
        let info = parse_github(&url, &github_pr()).unwrap();
        assert_eq!(info.source_branch, "topic");
        assert_eq!(info.target_branch, "main");
        assert_eq!(
            info.head_sha, "89abcdef0123456789abcdef0123456789abcdef",
            "the head, never merge_commit_sha"
        );

        let cases: Vec<(&str, serde_json::Value)> = vec![
            ("closed", {
                let mut p = github_pr();
                p["state"] = "closed".into();
                p
            }),
            ("merged", {
                let mut p = github_pr();
                p["merged"] = true.into();
                p
            }),
            ("fork", {
                let mut p = github_pr();
                p["head"]["repo"]["id"] = 10.into();
                p
            }),
            ("deleted head repo", {
                let mut p = github_pr();
                p["head"]["repo"] = serde_json::Value::Null;
                p
            }),
            ("wrong base repo", {
                let mut p = github_pr();
                p["base"]["repo"]["full_name"] = "Other/Repo".into();
                p
            }),
            ("wrong number", {
                let mut p = github_pr();
                p["number"] = 13.into();
                p
            }),
            ("bad sha", {
                let mut p = github_pr();
                p["head"]["sha"] = "short".into();
                p
            }),
            ("foreign html_url", {
                let mut p = github_pr();
                p["html_url"] = "https://github.com/Other/Repo/pull/12".into();
                p
            }),
        ];
        for (name, pr) in cases {
            let err = parse_github(&url, &pr).unwrap_err();
            assert!(matches!(err, HubError::Precondition(_)), "{name}: {err}");
        }
    }
}
