//! Methodology recipe commands — list what is on offer, install one.
//!
//! Both proxy through the in-process gRPC server, like every other data
//! command here. The install itself (step ordering, id-collision re-keying,
//! per-step reporting) lives in `packages/core` beside the recipe it executes,
//! so this layer only moves a choice one way and a report back.
//!
//! Deliberately thin: a partial install is a successful RPC carrying a report
//! that says `success: false`, not an error. The report is the only record of
//! how far the install got, and collapsing it into an error string would
//! discard exactly what the user needs to see.

use serde::{Deserialize, Serialize};
use tauri::State;
use tonic::Request;

use super::nodes::CommandError;
use crate::services::GrpcClient;
use nodespace_proto::nodespace::{InstallMethodologyRequest, ListMethodologiesRequest};

/// A recipe on offer, for the picker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Methodology {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// What one install step did.
///
/// Mirrors `nodespace_core::methodology::StepOutcome`, which is already
/// `#[serde(tag = "kind", rename_all = "camelCase")]`, so this deserializes
/// the report the daemon forwards without a second conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum StepOutcome {
    Created {
        id: String,
    },
    /// The requested id was taken, so the step landed under `created`
    /// instead. Surfaced to the user rather than silently resolved.
    Suffixed {
        requested: String,
        created: String,
    },
    Skipped,
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    pub label: String,
    pub outcome: StepOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub recipe_id: String,
    pub steps: Vec<StepReport>,
    pub success: bool,
}

/// List the methodology recipes this build ships.
#[tauri::command]
pub async fn list_methodologies(
    client: State<'_, GrpcClient>,
) -> Result<Vec<Methodology>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .list_methodologies(Request::new(ListMethodologiesRequest {}))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to list methodologies: {}", s.message()),
            code: "METHODOLOGY_SERVICE_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    Ok(resp
        .into_inner()
        .methodologies
        .into_iter()
        .map(|m| Methodology {
            id: m.id,
            name: m.name,
            description: m.description,
        })
        .collect())
}

/// Install a methodology recipe by id.
///
/// Returns the report even when a step failed — `success` says whether every
/// step landed, and `steps` says how far it got. Only a transport or decode
/// failure surfaces as `Err`.
#[tauri::command]
pub async fn install_methodology(
    client: State<'_, GrpcClient>,
    methodology_id: String,
) -> Result<InstallReport, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .install_methodology(Request::new(InstallMethodologyRequest {
            methodology_id: methodology_id.clone(),
        }))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to install methodology: {}", s.message()),
            code: "METHODOLOGY_INSTALL_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    serde_json::from_str(&resp.into_inner().report_json).map_err(|e| CommandError {
        message: format!("Could not read the install report: {e}"),
        code: "METHODOLOGY_REPORT_DECODE_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report crosses the wire as the JSON core emits. Pinned because a
    /// drift in either serde attribute would surface as a decode error at
    /// install time, with the install already done.
    #[test]
    fn install_report_deserializes_the_shape_core_emits() {
        let json = r#"{
            "recipeId": "linear",
            "success": true,
            "steps": [
                { "label": "Create `issue` schema", "outcome": { "kind": "created", "id": "issue" } },
                { "label": "Create `cycle` schema",
                  "outcome": { "kind": "suffixed", "requested": "cycle", "created": "cycle__2" } },
                { "label": "Install Play: x", "outcome": { "kind": "skipped" } },
                { "label": "Seed skill: y",
                  "outcome": { "kind": "failed", "message": "nope" } }
            ]
        }"#;

        let report: InstallReport = serde_json::from_str(json).expect("report should decode");
        assert_eq!(report.recipe_id, "linear");
        assert!(report.success);
        assert_eq!(report.steps.len(), 4);

        match &report.steps[1].outcome {
            StepOutcome::Suffixed { requested, created } => {
                assert_eq!(requested, "cycle");
                assert_eq!(created, "cycle__2");
            }
            other => panic!("expected a suffixed outcome, got {other:?}"),
        }
    }
}
