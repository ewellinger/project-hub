//! The last review state read from the forge for each role in review,
//! remembered in `<state_dir>/reviews/<feature>.json`.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{HubError, Result};
use crate::feature::{Feature, Stage};
use crate::hub::Hub;
use crate::manifest::write_json;
use crate::ops::status::parallel_map;
use crate::review::{ReviewUrl, parse_review_url};
use crate::review_state::{ReviewState, fetch_state};

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

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
