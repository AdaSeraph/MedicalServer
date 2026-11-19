use bevy_ecs::prelude::{Query, Res};

use crate::models::TriageStatus;

use super::components::{PatientIdentity, RedFlagAlert, TriageResponse};
use super::schedules::Stage4LlmOutcome;

pub fn is_emergency_120(query: Query<&RedFlagAlert>) -> bool {
    query
        .get_single()
        .map(|a| a.is_emergency_120())
        .unwrap_or(false)
}

pub fn not_emergency_120(query: Query<&RedFlagAlert>) -> bool {
    !is_emergency_120(query)
}

pub fn llm_succeeded(outcome: Option<Res<Stage4LlmOutcome>>) -> bool {
    matches!(outcome.as_deref(), Some(Stage4LlmOutcome::Success(_)))
}

pub fn llm_failed(outcome: Option<Res<Stage4LlmOutcome>>) -> bool {
    matches!(outcome.as_deref(), Some(Stage4LlmOutcome::AllFailed))
}

pub fn should_consolidate_memory(
    identity: Query<&PatientIdentity>,
    response: Query<&TriageResponse>,
) -> bool {
    let consented = identity
        .get_single()
        .map(|i| i.consent_long_term_memory)
        .unwrap_or(false);
    let completed = response
        .get_single()
        .map(|r| r.status == TriageStatus::Completed)
        .unwrap_or(false);
    consented && completed
}
