use bevy_ecs::prelude::{IntoSystemConfigs, Resource, Schedule, World};
use bevy_ecs::schedule::{ExecutorKind, ScheduleLabel};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::models::{EpidemicContext, RecalledL3Event};
use crate::storage::{KnowledgeSearchHit, PatientL2Record, SessionState};

use super::conditions::{
    is_emergency_120, llm_failed, llm_succeeded, not_emergency_120, should_consolidate_memory,
};
use super::resources::SharedTriageResources;
use super::systems::{
    stage1::{
        emergency_short_circuit_response_system, keyword_extraction_system, red_flag_guard_system,
    },
    stage2::memory_recall_inject_system,
    stage3::hybrid_rag_scoring_system,
    stage4::{fallback_rule_triage_system, slot_evaluation_system, slot_question_system},
    stage5::memory_consolidation_plan_system,
};

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage1Label;
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage2Label;
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage3Label;
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage4PlanLabel;
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage4FinalizeLabel;
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stage5Label;

#[derive(Resource, Debug, Clone, Default)]
pub struct Stage2FetchedData {
    pub prior_session: Option<SessionState>,
    pub l2_record: Option<PatientL2Record>,
    pub l3_events: Vec<RecalledL3Event>,
    pub active_epidemics: Vec<EpidemicContext>,
}

#[derive(Debug, Clone)]
pub struct Stage3Candidate {
    pub hit: KnowledgeSearchHit,
    pub raw_rerank_score: f32,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct Stage3RawCandidates {
    pub candidates: Vec<Stage3Candidate>,
}

#[derive(Resource, Debug, Clone, Default)]
pub struct Stage4DecisionScratch {
    pub should_converge: bool,
    pub target_slot: Option<String>,
    pub priority_hint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Stage4LlmJson {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub quick_options: Vec<String>,
    #[serde(default)]
    pub primary_department: Option<String>,
    #[serde(default)]
    pub secondary_departments: Vec<String>,
    #[serde(default)]
    pub urgency_level: Option<u8>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Resource, Debug, Clone, Default)]
pub enum Stage4LlmOutcome {
    #[default]
    Pending,
    Success(Stage4LlmJson),
    AllFailed,
}

pub struct TriageSchedules {
    pub stage1: Schedule,
    pub stage2: Schedule,
    pub stage3: Schedule,
    pub stage4_plan: Schedule,
    pub stage4_finalize: Schedule,
    pub stage5: Schedule,
}

impl Default for TriageSchedules {
    fn default() -> Self {
        Self::new()
    }
}

impl TriageSchedules {
    pub fn new() -> Self {
        let mut stage1 = Schedule::new(Stage1Label);
        stage1.set_executor_kind(ExecutorKind::SingleThreaded);
        stage1.add_systems(
            (
                keyword_extraction_system,
                red_flag_guard_system,
                emergency_short_circuit_response_system.run_if(is_emergency_120),
            )
                .chain(),
        );

        let mut stage2 = Schedule::new(Stage2Label);
        stage2.set_executor_kind(ExecutorKind::SingleThreaded);
        stage2.add_systems(
            (
                memory_recall_inject_system.run_if(not_emergency_120),
                red_flag_guard_system.run_if(not_emergency_120),
                emergency_short_circuit_response_system.run_if(is_emergency_120),
            )
                .chain(),
        );

        let mut stage3 = Schedule::new(Stage3Label);
        stage3.set_executor_kind(ExecutorKind::SingleThreaded);
        stage3.add_systems(hybrid_rag_scoring_system.run_if(not_emergency_120));

        let mut stage4_plan = Schedule::new(Stage4PlanLabel);
        stage4_plan.set_executor_kind(ExecutorKind::SingleThreaded);
        stage4_plan.add_systems(slot_evaluation_system.run_if(not_emergency_120));

        let mut stage4_finalize = Schedule::new(Stage4FinalizeLabel);
        stage4_finalize.set_executor_kind(ExecutorKind::SingleThreaded);
        stage4_finalize.add_systems((
            slot_question_system.run_if(not_emergency_120).run_if(llm_succeeded),
            fallback_rule_triage_system
                .run_if(not_emergency_120)
                .run_if(llm_failed),
        ));

        let mut stage5 = Schedule::new(Stage5Label);
        stage5.set_executor_kind(ExecutorKind::SingleThreaded);
        stage5.add_systems(memory_consolidation_plan_system.run_if(should_consolidate_memory));

        Self {
            stage1,
            stage2,
            stage3,
            stage4_plan,
            stage4_finalize,
            stage5,
        }
    }
}

pub struct PooledEcsSlot {
    pub world: World,
    pub schedules: TriageSchedules,
}

impl PooledEcsSlot {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            schedules: TriageSchedules::new(),
        }
    }

    pub fn reset_for_reuse(&mut self) {
        self.world.clear_entities();
        self.world.remove_resource::<SharedTriageResources>();
        self.world.remove_resource::<Stage2FetchedData>();
        self.world.remove_resource::<Stage3RawCandidates>();
        self.world.remove_resource::<Stage4DecisionScratch>();
        self.world.remove_resource::<Stage4LlmOutcome>();
    }
}

impl Default for PooledEcsSlot {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WorldPool {
    slots: Mutex<Vec<PooledEcsSlot>>,
    max_pooled: usize,
}

impl WorldPool {
    pub fn new(max_pooled: usize) -> Self {
        Self {
            slots: Mutex::new(Vec::with_capacity(max_pooled.min(64))),
            max_pooled,
        }
    }

    pub fn acquire(&self) -> PooledWorldGuard<'_> {
        let mut slot = self
            .slots
            .lock()
            .pop()
            .unwrap_or_default();
        slot.reset_for_reuse();
        PooledWorldGuard {
            pool: self,
            slot: Some(slot),
        }
    }

    pub fn pooled_len(&self) -> usize {
        self.slots.lock().len()
    }
}

pub struct PooledWorldGuard<'a> {
    pool: &'a WorldPool,
    slot: Option<PooledEcsSlot>,
}

impl<'a> PooledWorldGuard<'a> {
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.slot.as_mut().expect("pooled slot present").world
    }

    pub fn schedules_mut(&mut self) -> &mut TriageSchedules {
        &mut self.slot.as_mut().expect("pooled slot present").schedules
    }

    pub fn run_stage1(&mut self) {
        let s = self.slot.as_mut().expect("pooled slot present");
        s.schedules.stage1.run(&mut s.world);
    }

    pub fn run_stage2(&mut self) {
        let s = self.slot.as_mut().expect("pooled slot present");
        s.schedules.stage2.run(&mut s.world);
    }

    pub fn run_stage3(&mut self) {
        let s = self.slot.as_mut().expect("pooled slot present");
        s.schedules.stage3.run(&mut s.world);
    }

    pub fn run_stage4_plan(&mut self) {
        let s = self.slot.as_mut().expect("pooled slot present");
        s.schedules.stage4_plan.run(&mut s.world);
    }

    pub fn run_stage4_finalize(&mut self) {
        let s = self.slot.as_mut().expect("pooled slot present");
        s.schedules.stage4_finalize.run(&mut s.world);
    }

    pub fn run_stage5(&mut self) {
        let s = self.slot.as_mut().expect("pooled slot present");
        s.schedules.stage5.run(&mut s.world);
    }
}

impl Drop for PooledWorldGuard<'_> {
    fn drop(&mut self) {
        if let Some(mut slot) = self.slot.take() {
            slot.reset_for_reuse();
            let mut guard = self.pool.slots.lock();
            if guard.len() < self.pool.max_pooled {
                guard.push(slot);
            }
        }
    }
}
