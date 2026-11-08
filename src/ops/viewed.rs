//! `hub feature viewed ROLE`: read the role's request from the forge and
//! record its head and comment count as the point the user last looked.

use crate::error::{HubError, Result};
use crate::feature::{Feature, short_sha};
use crate::hub::Hub;
use crate::ops::review_state::{now_secs, refresh, store, targets};
use crate::review::Provider;
use crate::review_state::Viewed;

/// Never changes the feature record; writes only the reviews file.
pub fn viewed(hub: &Hub, feature: &Feature, role: &str) -> Result<Vec<String>> {
    feature.require_open()?;
    hub.manifest.require_repo(role)?;
    let target = targets(feature)
        .into_iter()
        .find(|t| t.role == role)
        .ok_or_else(|| {
            HubError::Precondition(format!(
                "role '{role}' in feature '{}' is not in review with a merge/pull request URL",
                feature.name
            ))
        })?;
    let before = now_secs();
    let (mut states, warnings) = refresh(hub, feature);
    let fresh = states
        .get_mut(role)
        .filter(|s| s.read_at >= before)
        .ok_or_else(|| {
            let mine = format!("for {role}:");
            let why = warnings
                .iter()
                .find(|w| w.contains(&mine))
                .or(warnings.first())
                .cloned()
                .unwrap_or_else(|| format!("review state not read for {role}"));
            HubError::Precondition(why)
        })?;
    let head = fresh.fields.head.clone();
    let comments = fresh.fields.comments;
    fresh.viewed = Some(Viewed {
        at: fresh.read_at,
        head: head.clone(),
        comments,
    });
    store(hub, feature, &mut states, Some(role))?;
    let request = match target.url.provider {
        Provider::GitLab => format!("!{}", target.url.number),
        Provider::GitHub => format!("#{}", target.url.number),
    };
    let head = head.as_deref().map_or("?", short_sha);
    let comments = comments.map_or("?".to_string(), |n| n.to_string());
    Ok(vec![format!(
        "{role}: viewed {request} head {head}, {comments} comments"
    )])
}
