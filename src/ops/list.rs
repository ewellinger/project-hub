use serde::Serialize;

use crate::error::{HubError, Result};
use crate::feature::{FeatureStatus, Stage};
use crate::hub::Hub;
use crate::ops::table;

#[derive(Debug, Clone, Serialize)]
pub struct RoleSummary {
    pub role: String,
    pub branch: String,
    pub stage: Stage,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureSummary {
    pub name: String,
    pub checkout: String,
    pub status: FeatureStatus,
    /// Latest change per role, in manifest order.
    pub roles: Vec<RoleSummary>,
}

/// Open features first, then finished, each group by name.
pub fn list(hub: &Hub) -> Result<Vec<FeatureSummary>> {
    let mut summaries: Vec<FeatureSummary> = hub
        .list_features()?
        .into_iter()
        .map(|f| {
            let mut roles: Vec<RoleSummary> = hub
                .manifest
                .repos
                .iter()
                .filter_map(|repo| {
                    f.last_change(&repo.role).map(|c| RoleSummary {
                        role: repo.role.clone(),
                        branch: c.branch.clone(),
                        stage: c.stage,
                    })
                })
                .collect();
            // Roles removed from hub.json after the feature used them still show, last.
            for change in &f.changes {
                if hub.manifest.repo(&change.role).is_none()
                    && !roles.iter().any(|r| r.role == change.role)
                {
                    let last = f.last_change(&change.role).expect("change exists");
                    roles.push(RoleSummary {
                        role: change.role.clone(),
                        branch: last.branch.clone(),
                        stage: last.stage,
                    });
                }
            }
            FeatureSummary {
                name: f.name,
                checkout: f.checkout,
                status: f.status,
                roles,
            }
        })
        .collect();
    summaries.sort_by_key(|s| s.status != FeatureStatus::Open);
    Ok(summaries)
}

pub fn render_table(summaries: &[FeatureSummary]) -> Vec<String> {
    let rows: Vec<Vec<String>> = summaries
        .iter()
        .map(|s| {
            let roles = s
                .roles
                .iter()
                .map(|r| format!("{}:{}", r.role, r.stage.as_str()))
                .collect::<Vec<_>>()
                .join(" ");
            let status = match s.status {
                FeatureStatus::Open => "open",
                FeatureStatus::Finished => "finished",
            };
            vec![
                s.name.clone(),
                status.to_string(),
                s.checkout.clone(),
                roles,
            ]
        })
        .collect();
    table(&["FEATURE", "STATUS", "SESSION", "ROLES"], &rows)
}

pub fn render_json(summaries: &[FeatureSummary]) -> Result<String> {
    serde_json::to_string_pretty(summaries)
        .map_err(|e| HubError::Precondition(format!("serializing features: {e}")))
}
