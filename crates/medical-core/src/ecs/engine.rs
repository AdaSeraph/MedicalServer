use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::llm::{
    LlmMessage, LlmRequest, LlmStreamChunk, ModelTier, RerankCandidate,
};
use crate::models::{RedFlagLevel, TargetPopulation, TriageStatus};
use crate::privacy::scrub_pii;
use crate::storage::{
    KnowledgeSearchHit, KnowledgeSearchQuery, KnowledgeVectorFilter, SessionSlotsSnapshot,
    SessionState, SessionTurn, TriageAuditLog,
};

use super::components::{
    ConsolidationPlan, ExtractedSymptoms, MemoryRecall, PatientIdentity, PatientProfile,
    RagEvidence, RawUtterance, RedFlagAlert, SessionMemory, StageTiming, TriageResponse,
    TriageSlots, TriageTrace,
};
use super::resources::{SharedTriageResources, TriageResources};
use super::schedules::{
    Stage2FetchedData, Stage3Candidate, Stage3RawCandidates, Stage4DecisionScratch, Stage4LlmJson,
    Stage4LlmOutcome, WorldPool,
};
use super::systems::stage5::{build_consolidation_input, execute_memory_consolidation};

#[derive(Debug, Clone)]
pub struct TriageRequest {
    pub session_id: String,
    pub patient_id: String,
    pub tenant_id: String,
    pub region_code: Option<String>,
    pub utterance: String,
    pub target_population: Vec<TargetPopulation>,
    pub consent_long_term_memory: Option<bool>,
    pub prior_session: Option<SessionState>,
    pub stream_tx: Option<mpsc::Sender<TriageStreamEvent>>,
}

impl TriageRequest {
    pub fn simple(
        session_id: impl Into<String>,
        patient_id: impl Into<String>,
        utterance: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            patient_id: patient_id.into(),
            tenant_id: "default".to_string(),
            region_code: None,
            utterance: utterance.into(),
            target_population: vec![TargetPopulation::Adult],
            consent_long_term_memory: None,
            prior_session: None,
            stream_tx: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum TriageStreamEvent {
    Delta { delta: String, provider_id: String },
    Result(TriageResponse),
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageExecutionResult {
    pub response: TriageResponse,
    pub trace: TriageTrace,
}

#[derive(Debug, thiserror::Error)]
pub enum TriageEngineError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("ecs internal error: {0}")]
    Internal(String),
}

pub struct TriageEngine {
    resources: Arc<ArcSwap<TriageResources>>,
    pool: WorldPool,
}

impl TriageEngine {
    pub fn new(resources: Arc<ArcSwap<TriageResources>>) -> Self {
        Self {
            resources,
            pool: WorldPool::new(64),
        }
    }

    pub fn with_pool_size(resources: Arc<ArcSwap<TriageResources>>, max_pooled: usize) -> Self {
        Self {
            resources,
            pool: WorldPool::new(max_pooled.max(1)),
        }
    }

    pub fn resources(&self) -> Arc<TriageResources> {
        self.resources.load_full()
    }

    pub fn reload_resources(&self, new_resources: TriageResources) {
        self.resources.store(Arc::new(new_resources));
    }

    pub async fn run_triage(
        &self,
        req: TriageRequest,
    ) -> Result<TriageExecutionResult, TriageEngineError> {
        if req.utterance.trim().is_empty() {
            return Err(TriageEngineError::InvalidRequest(
                "utterance must not be empty".to_string(),
            ));
        }

        let total_started = Instant::now();
        let res = self.resources.load_full();
        let scrubbed_utterance = scrub_pii(&req.utterance).scrubbed_text;
        let max_turns = res.config.max_turns.clamp(1, 4);

        let mut guard = self.pool.acquire();
        let entity = {
            let world = guard.world_mut();
            world.insert_resource(SharedTriageResources(res.clone()));
            world
                .spawn((
                    PatientIdentity {
                        session_id: req.session_id.clone(),
                        patient_id: req.patient_id.clone(),
                        tenant_id: req.tenant_id.clone(),
                        region_code: req.region_code.clone(),
                        target_population: if req.target_population.is_empty() {
                            vec![TargetPopulation::Adult]
                        } else {
                            req.target_population.clone()
                        },
                        consent_long_term_memory: req.consent_long_term_memory.unwrap_or(false),
                    },
                    RawUtterance {
                        text: req.utterance.clone(),
                        scrubbed_text: scrubbed_utterance.clone(),
                        received_at: Utc::now(),
                    },
                    PatientProfile::default(),
                    SessionMemory::default(),
                    MemoryRecall::default(),
                    ExtractedSymptoms::default(),
                    RedFlagAlert::default(),
                    RagEvidence::default(),
                    TriageSlots {
                        turn: 1,
                        max_turns,
                        ..Default::default()
                    },
                    TriageResponse {
                        status: TriageStatus::FollowUp,
                        session_id: req.session_id.clone(),
                        red_flag: RedFlagLevel::None,
                        turn: 1,
                        max_turns,
                        question: None,
                        quick_options: Vec::new(),
                        seasonal_context_applied: Vec::new(),
                        triage_result: None,
                        citations: Vec::new(),
                        is_fallback: false,
                    },
                    ConsolidationPlan::default(),
                ))
                .id()
        };

        let mut stage_timings: Vec<StageTiming> = Vec::with_capacity(5);
        let mut llm_calls_made = 0_u32;

        // ==================== STAGE 1: CPU Keyword + Red-Flag Guard ====================
        let s1_start = Instant::now();
        guard.run_stage1();
        stage_timings.push(StageTiming {
            stage: "stage1_red_flag_guard".to_string(),
            duration_ms: s1_start.elapsed().as_millis() as u64,
            skipped: false,
        });

        let stage1_emergency = {
            let world = guard.world_mut();
            world
                .get::<RedFlagAlert>(entity)
                .map(|a| a.is_emergency_120())
                .unwrap_or(false)
        };

        if stage1_emergency {
            for name in [
                "stage2_memory_recall",
                "stage3_hybrid_rag",
                "stage4_slot_decision",
                "stage5_consolidation",
            ] {
                stage_timings.push(StageTiming {
                    stage: name.to_string(),
                    duration_ms: 0,
                    skipped: true,
                });
            }

            let trace = extract_trace(
                guard.world_mut(),
                entity,
                stage_timings,
                llm_calls_made,
                false,
                false,
                total_started.elapsed().as_millis() as u64,
            )?;
            let response = trace.triage_response.clone();

            if let Some(tx) = &req.stream_tx {
                let _ = tx.send(TriageStreamEvent::Result(response.clone())).await;
            }
            record_audit_log(&res, &req, &response, None, None, 0, 0, total_started).await;
            return Ok(TriageExecutionResult { response, trace });
        }

        // ==================== STAGE 2: Parallel L1/L2/L3 + Epidemics ====================
        let s2_start = Instant::now();
        let today = Utc::now().date_naive();

        let (prior_res, l2_res, q_vec_res, db_epi_res) = tokio::join!(
            async {
                if let Some(prior) = &req.prior_session {
                    Ok(Some(prior.clone()))
                } else {
                    res.clients.stores.session_cache.get_session(&req.session_id).await
                }
            },
            res.clients
                .stores
                .patient_memory
                .get_l2_profile(&req.patient_id),
            res.clients.embedding.embed_one(&scrubbed_utterance),
            async {
                if res.epidemics.is_empty() {
                    res.clients
                        .stores
                        .epidemics
                        .list_active(req.region_code.as_deref(), None, today)
                        .await
                } else {
                    Ok(Vec::new())
                }
            }
        );

        let prior_session = prior_res.ok().flatten();
        let raw_l2 = l2_res.ok().flatten();
        let effective_consent = req
            .consent_long_term_memory
            .or_else(|| raw_l2.as_ref().map(|r| r.consent_long_term_memory))
            .unwrap_or(false);

        let l2_record = if effective_consent { raw_l2 } else { None };
        let l3_events = if effective_consent {
            if let Ok(q_vec) = q_vec_res {
                res.clients
                    .stores
                    .patient_l3_vectors
                    .recall_similar_events(&req.patient_id, &q_vec, 3)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let active_epidemics = if !res.epidemics.is_empty() {
            res.epidemics
                .iter()
                .filter(|c| {
                    c.active
                        && c.valid_from <= today
                        && c.valid_to >= today
                        && req
                            .region_code
                            .as_deref()
                            .map(|r| c.region_code == r)
                            .unwrap_or(true)
                })
                .cloned()
                .collect()
        } else {
            db_epi_res.unwrap_or_default()
        };

        {
            let world = guard.world_mut();
            if let Some(mut id) = world.get_mut::<PatientIdentity>(entity) {
                id.consent_long_term_memory = effective_consent;
            }
            world.insert_resource(Stage2FetchedData {
                prior_session,
                l2_record,
                l3_events,
                active_epidemics,
            });
        }

        guard.run_stage2();

        let stage2_emergency = {
            let world = guard.world_mut();
            world
                .get::<RedFlagAlert>(entity)
                .map(|a| a.is_emergency_120())
                .unwrap_or(false)
        };

        if stage2_emergency {
            stage_timings.push(StageTiming {
                stage: "stage2_memory_recall".to_string(),
                duration_ms: s2_start.elapsed().as_millis() as u64,
                skipped: false,
            });
            for name in [
                "stage3_hybrid_rag",
                "stage4_slot_decision",
                "stage5_consolidation",
            ] {
                stage_timings.push(StageTiming {
                    stage: name.to_string(),
                    duration_ms: 0,
                    skipped: true,
                });
            }
            let trace = extract_trace(
                guard.world_mut(),
                entity,
                stage_timings,
                llm_calls_made,
                false,
                false,
                total_started.elapsed().as_millis() as u64,
            )?;
            let response = trace.triage_response.clone();
            if let Some(tx) = &req.stream_tx {
                let _ = tx.send(TriageStreamEvent::Result(response.clone())).await;
            }
            record_audit_log(&res, &req, &response, None, None, 0, 0, total_started).await;
            return Ok(TriageExecutionResult { response, trace });
        }

        // Optional Fast-Tier LLM Query Rewrite
        let deterministic_query = {
            let world = guard.world_mut();
            world
                .get::<SessionMemory>(entity)
                .and_then(|s| s.rewritten_query.clone())
                .unwrap_or_else(|| scrubbed_utterance.clone())
        };

        let mut final_query = deterministic_query.clone();
        if res.config.use_fast_query_rewrite && !res.model_router.router_config().fast_chain.is_empty() {
            let rewrite_req = LlmRequest {
                messages: vec![
                    LlmMessage::system("你是医学检索Query改写助手。将患者主诉与病史浓缩为用于医学知识库检索的简洁中文检索词，仅输出JSON: {\"rewritten_query\":\"...\"}"),
                    LlmMessage::user(deterministic_query.clone()),
                ],
                tier: ModelTier::Fast,
                temperature: Some(0.1),
                max_tokens: Some(128),
                response_format_json: true,
                json_schema: None,
                timeout_ms: Some(1500),
            };
            if let Ok(resp) = res.model_router.complete_json(&rewrite_req).await {
                llm_calls_made += 1;
                #[derive(Deserialize)]
                struct RewriteOut {
                    rewritten_query: Option<String>,
                }
                if let Ok(parsed) = serde_json::from_str::<RewriteOut>(&resp.content)
                    && let Some(q) = parsed.rewritten_query
                    && !q.trim().is_empty()
                {
                    final_query = scrub_pii(&q).scrubbed_text;
                    let world = guard.world_mut();
                    if let Some(mut sm) = world.get_mut::<SessionMemory>(entity) {
                        sm.rewritten_query = Some(final_query.clone());
                        sm.query_rewrite_used_llm = true;
                    }
                }
            }
        }

        stage_timings.push(StageTiming {
            stage: "stage2_memory_recall".to_string(),
            duration_ms: s2_start.elapsed().as_millis() as u64,
            skipped: false,
        });

        // ==================== STAGE 3: Hybrid RAG + Rerank + Seasonal Boost ====================
        let s3_start = Instant::now();
        let (target_pop, normalized_terms) = {
            let world = guard.world_mut();
            let id = world.get::<PatientIdentity>(entity).expect("identity exists");
            let sym = world.get::<ExtractedSymptoms>(entity).expect("symptoms exist");
            (id.target_population.clone(), sym.normalized_terms.clone())
        };

        let q_vec = res.clients.embedding.embed_one(&final_query).await.ok();
        let search_query = KnowledgeSearchQuery {
            query_text: final_query.clone(),
            query_vector: q_vec.clone(),
            keywords: normalized_terms,
            target_population: Some(target_pop.clone()),
            department: None,
            tenant_namespace: Some(req.tenant_id.clone()),
            top_k: res.config.rag_top_k.max(5),
        };

        let (search_res, vec_res) = tokio::join!(
            res.clients.stores.knowledge_search.search(&search_query),
            async {
                if let Some(ref v) = q_vec {
                    res.clients
                        .stores
                        .knowledge_vectors
                        .search_vectors(
                            v,
                            &KnowledgeVectorFilter {
                                target_population: Some(target_pop),
                                department: None,
                                tenant_namespace: Some(req.tenant_id.clone()),
                            },
                            res.config.rag_top_k.max(5),
                        )
                        .await
                } else {
                    Ok(Vec::new())
                }
            }
        );

        let mut merged_hits: HashMap<String, KnowledgeSearchHit> = HashMap::new();
        for hit in search_res.unwrap_or_default() {
            merged_hits.insert(hit.doc_id.clone(), hit);
        }
        for hit in vec_res.unwrap_or_default() {
            merged_hits
                .entry(hit.doc_id.clone())
                .and_modify(|existing| {
                    if existing.vector_score.is_none() {
                        existing.vector_score = hit.vector_score;
                    }
                    existing.rrf_score = (existing.rrf_score + hit.rrf_score) * 0.5;
                })
                .or_insert(hit);
        }

        let hits_vec: Vec<KnowledgeSearchHit> = merged_hits.into_values().collect();
        let rerank_candidates: Vec<RerankCandidate> = hits_vec
            .iter()
            .map(|h| RerankCandidate {
                doc_id: h.doc_id.clone(),
                text: format!("{} {} {}", h.disease_name, h.key_symptoms.join(" "), h.content),
                initial_score: h.rrf_score,
            })
            .collect();

        let rerank_scores = res
            .clients
            .rerank
            .rerank(&final_query, &rerank_candidates)
            .await
            .unwrap_or_else(|_| hits_vec.iter().map(|h| h.rrf_score).collect());

        let raw_candidates: Vec<Stage3Candidate> = hits_vec
            .into_iter()
            .enumerate()
            .map(|(idx, hit)| {
                let raw_score = rerank_scores.get(idx).copied().unwrap_or(hit.rrf_score);
                Stage3Candidate {
                    hit,
                    raw_rerank_score: raw_score,
                }
            })
            .collect();

        {
            let world = guard.world_mut();
            world.insert_resource(Stage3RawCandidates {
                candidates: raw_candidates,
            });
        }
        guard.run_stage3();

        stage_timings.push(StageTiming {
            stage: "stage3_hybrid_rag".to_string(),
            duration_ms: s3_start.elapsed().as_millis() as u64,
            skipped: false,
        });

        // ==================== STAGE 4: Slot Evaluation -> Reasoning LLM -> Finalize ====================
        let s4_start = Instant::now();
        {
            let world = guard.world_mut();
            world.insert_resource(Stage4DecisionScratch::default());
            world.insert_resource(Stage4LlmOutcome::Pending);
        }
        guard.run_stage4_plan();

        let (reasoning_req, slots_snap) = {
            let world = guard.world_mut();
            let scratch = world.resource::<Stage4DecisionScratch>();
            let slots = world.get::<TriageSlots>(entity).expect("slots exist");
            let evidence = world.get::<RagEvidence>(entity).expect("evidence exists");
            let profile = world.get::<PatientProfile>(entity).expect("profile exists");
            let recall = world.get::<MemoryRecall>(entity).expect("recall exists");
            let session_mem = world.get::<SessionMemory>(entity).expect("session exists");

            let req_built = build_reasoning_request(
                &scrubbed_utterance,
                scratch,
                slots,
                evidence,
                profile,
                recall,
                session_mem,
            );
            (req_built, slots.clone())
        };

        let mut used_provider_id = None;
        let mut used_model = None;
        let mut prompt_tokens = 0_u32;
        let mut completion_tokens = 0_u32;

        let llm_res = if let Some(outer_tx) = &req.stream_tx {
            let (llm_tx, mut llm_rx) = mpsc::channel::<LlmStreamChunk>(64);
            let fwd_tx = outer_tx.clone();
            let fwd_handle = tokio::spawn(async move {
                while let Some(chunk) = llm_rx.recv().await {
                    if !chunk.delta.is_empty() {
                        let _ = fwd_tx
                            .send(TriageStreamEvent::Delta {
                                delta: chunk.delta,
                                provider_id: chunk.provider_id,
                            })
                            .await;
                    }
                }
            });
            let out = res.model_router.chat_stream(&reasoning_req, llm_tx).await;
            let _ = fwd_handle.await;
            out
        } else {
            res.model_router.complete_json(&reasoning_req).await
        };

        let outcome = match llm_res {
            Ok(resp) => {
                llm_calls_made += 1;
                used_provider_id = Some(resp.provider_id.clone());
                used_model = Some(resp.model.clone());
                prompt_tokens = resp.usage.prompt_tokens;
                completion_tokens = resp.usage.completion_tokens;

                match parse_stage4_json(&resp.content) {
                    Some(parsed) => Stage4LlmOutcome::Success(parsed),
                    None => Stage4LlmOutcome::AllFailed,
                }
            }
            Err(_) => Stage4LlmOutcome::AllFailed,
        };

        {
            let world = guard.world_mut();
            world.insert_resource(outcome);
        }
        guard.run_stage4_finalize();

        // Update L1 SessionCache
        let (final_response, turns_to_save, updated_slots) = {
            let world = guard.world_mut();
            let resp = world
                .get::<TriageResponse>(entity)
                .expect("response exists")
                .clone();
            let sm = world
                .get::<SessionMemory>(entity)
                .expect("session exists")
                .clone();
            let sl = world.get::<TriageSlots>(entity).expect("slots exist").clone();
            (resp, sm.turns, sl)
        };

        let mut session_turns: Vec<SessionTurn> = turns_to_save
            .into_iter()
            .map(|t| SessionTurn {
                role: t.role,
                content: t.content,
                timestamp: t.timestamp,
            })
            .collect();
        let assistant_text = final_response
            .question
            .clone()
            .or_else(|| {
                final_response
                    .triage_result
                    .as_ref()
                    .map(|c| c.summary.clone())
            })
            .unwrap_or_default();
        if !assistant_text.is_empty() {
            session_turns.push(SessionTurn {
                role: "assistant".to_string(),
                content: assistant_text,
                timestamp: Utc::now(),
            });
        }

        let session_state = SessionState {
            session_id: req.session_id.clone(),
            patient_id: req.patient_id.clone(),
            turns: session_turns,
            slots: SessionSlotsSnapshot {
                chief_complaint: updated_slots.chief_complaint,
                duration: updated_slots.duration,
                severity_or_temp: updated_slots.temperature_or_vitals,
                accompanying_symptoms: updated_slots.accompanying_symptoms,
                exposure_history: updated_slots.contact_history,
                turn_count: slots_snap.turn,
            },
            updated_at: Utc::now(),
        };
        let _ = res
            .clients
            .stores
            .session_cache
            .save_session(&session_state, res.config.session_ttl_secs)
            .await;

        if let Some(tx) = &req.stream_tx {
            let _ = tx
                .send(TriageStreamEvent::Result(final_response.clone()))
                .await;
        }

        stage_timings.push(StageTiming {
            stage: "stage4_slot_decision".to_string(),
            duration_ms: s4_start.elapsed().as_millis() as u64,
            skipped: false,
        });

        // ==================== STAGE 5: Consent-Gated Long-Term Memory Consolidation ====================
        let s5_start = Instant::now();
        guard.run_stage5();

        let (should_consolidate, consolidation_input) = {
            let world = guard.world_mut();
            let plan = world
                .get::<ConsolidationPlan>(entity)
                .expect("plan exists");
            if plan.should_consolidate {
                let id = world.get::<PatientIdentity>(entity).expect("id");
                let raw = world.get::<RawUtterance>(entity).expect("raw");
                let prof = world.get::<PatientProfile>(entity).expect("prof");
                let sym = world.get::<ExtractedSymptoms>(entity).expect("sym");
                let sl = world.get::<TriageSlots>(entity).expect("sl");
                let resp = world.get::<TriageResponse>(entity).expect("resp");
                (
                    true,
                    build_consolidation_input(id, raw, prof, sym, sl, resp),
                )
            } else {
                (false, None)
            }
        };

        let mut consolidation_completed = false;
        if should_consolidate && let Some(input) = consolidation_input {
            if res.config.await_consolidation {
                consolidation_completed = execute_memory_consolidation(res.clone(), input).await;
            } else {
                let res_bg = res.clone();
                tokio::spawn(async move {
                    let _ = execute_memory_consolidation(res_bg, input).await;
                });
            }
        }

        stage_timings.push(StageTiming {
            stage: "stage5_consolidation".to_string(),
            duration_ms: s5_start.elapsed().as_millis() as u64,
            skipped: !should_consolidate,
        });

        let trace = extract_trace(
            guard.world_mut(),
            entity,
            stage_timings,
            llm_calls_made,
            should_consolidate,
            consolidation_completed,
            total_started.elapsed().as_millis() as u64,
        )?;

        record_audit_log(
            &res,
            &req,
            &final_response,
            used_provider_id,
            used_model,
            prompt_tokens,
            completion_tokens,
            total_started,
        )
        .await;

        Ok(TriageExecutionResult {
            response: final_response,
            trace,
        })
    }
}

fn build_reasoning_request(
    scrubbed_utterance: &str,
    scratch: &Stage4DecisionScratch,
    slots: &TriageSlots,
    evidence: &RagEvidence,
    profile: &PatientProfile,
    recall: &MemoryRecall,
    session_mem: &SessionMemory,
) -> LlmRequest {
    let evidence_text: String = evidence
        .items
        .iter()
        .take(3)
        .map(|i| {
            format!(
                "- [{}][{}] 科室:{} 权威度:{:.2} 综合分:{:.3} 内容:{}",
                i.doc_id, i.disease_name, i.department, i.authority_level, i.final_score, i.snippet
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let epi_text: String = recall
        .active_epidemics
        .iter()
        .map(|e| {
            format!(
                "{}(强度{:.2}, 优先追问提示:{})",
                e.disease_name, e.intensity, e.priority_question_hint
            )
        })
        .collect::<Vec<_>>()
        .join("；");

    let l2_text = profile
        .l2_profile
        .as_ref()
        .map(|l2| {
            format!(
                "年龄段:{}, 慢病:{:?}, 过敏:{:?}",
                l2.age_group, l2.chronic_diseases, l2.allergies
            )
        })
        .unwrap_or_else(|| "无长期档案".to_string());

    let turns_text: String = session_mem
        .turns
        .iter()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n");

    let action_hint = if scratch.should_converge {
        "必须收敛并输出最终分诊结果(action=\"completed\")"
    } else {
        "若信息仍不足可继续短追问(action=\"follow_up\")，追问正文严格<=30字且附带2~4个快捷选项；若信息充分也可直接输出completed"
    };

    let sys_prompt = "你是医院智能预检分诊AI。严禁输出确诊结论和具体处方药剂量。严格按JSON输出：{\"action\":\"follow_up|completed\",\"question\":\"...\",\"quick_options\":[\"...\"],\"primary_department\":\"...\",\"secondary_departments\":[],\"urgency_level\":2,\"summary\":\"...\"}";
    let user_prompt = format!(
        "当前轮次:{}/{}\n患者本轮主诉:{}\n对话历史:\n{}\nL2画像:{}\n季节流行病:{}\n优先追问提示:{:?}\n目标槽位:{:?}\n决策要求:{}\n知识库依据:\n{}",
        slots.turn,
        slots.max_turns,
        scrubbed_utterance,
        turns_text,
        l2_text,
        epi_text,
        scratch.priority_hint,
        scratch.target_slot,
        action_hint,
        evidence_text
    );

    LlmRequest {
        messages: vec![LlmMessage::system(sys_prompt), LlmMessage::user(user_prompt)],
        tier: ModelTier::Reasoning,
        temperature: Some(0.2),
        max_tokens: Some(512),
        response_format_json: true,
        json_schema: None,
        timeout_ms: None,
    }
}

fn parse_stage4_json(raw: &str) -> Option<Stage4LlmJson> {
    let trimmed = raw.trim();
    let cleaned = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim())
        .and_then(|s| s.strip_suffix("```"))
        .map(|s| s.trim())
        .unwrap_or(trimmed);

    if let Ok(parsed) = serde_json::from_str::<Stage4LlmJson>(cleaned) {
        return Some(parsed);
    }
    // Fallback: extract substring from first '{' to last '}'
    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}'))
        && start < end
    {
        return serde_json::from_str::<Stage4LlmJson>(&cleaned[start..=end]).ok();
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn extract_trace(
    world: &mut bevy_ecs::prelude::World,
    entity: bevy_ecs::prelude::Entity,
    stage_timings: Vec<StageTiming>,
    llm_calls_made: u32,
    consolidation_scheduled: bool,
    consolidation_completed: bool,
    total_duration_ms: u64,
) -> Result<TriageTrace, TriageEngineError> {
    let e_ref = world.entity(entity);
    let err = |name: &str| {
        TriageEngineError::Internal(format!("missing component {name} on TriageSession entity"))
    };
    Ok(TriageTrace {
        patient_identity: e_ref
            .get::<PatientIdentity>()
            .cloned()
            .ok_or_else(|| err("PatientIdentity"))?,
        raw_utterance: e_ref
            .get::<RawUtterance>()
            .cloned()
            .ok_or_else(|| err("RawUtterance"))?,
        patient_profile: e_ref
            .get::<PatientProfile>()
            .cloned()
            .ok_or_else(|| err("PatientProfile"))?,
        session_memory: e_ref
            .get::<SessionMemory>()
            .cloned()
            .ok_or_else(|| err("SessionMemory"))?,
        memory_recall: e_ref
            .get::<MemoryRecall>()
            .cloned()
            .ok_or_else(|| err("MemoryRecall"))?,
        extracted_symptoms: e_ref
            .get::<ExtractedSymptoms>()
            .cloned()
            .ok_or_else(|| err("ExtractedSymptoms"))?,
        red_flag_alert: e_ref
            .get::<RedFlagAlert>()
            .cloned()
            .ok_or_else(|| err("RedFlagAlert"))?,
        rag_evidence: e_ref
            .get::<RagEvidence>()
            .cloned()
            .ok_or_else(|| err("RagEvidence"))?,
        triage_slots: e_ref
            .get::<TriageSlots>()
            .cloned()
            .ok_or_else(|| err("TriageSlots"))?,
        triage_response: e_ref
            .get::<TriageResponse>()
            .cloned()
            .ok_or_else(|| err("TriageResponse"))?,
        stage_timings,
        llm_calls_made,
        consolidation_scheduled,
        consolidation_completed,
        total_duration_ms,
    })
}

#[allow(clippy::too_many_arguments)]
async fn record_audit_log(
    res: &TriageResources,
    req: &TriageRequest,
    resp: &TriageResponse,
    provider_id: Option<String>,
    model_name: Option<String>,
    prompt_tokens: u32,
    completion_tokens: u32,
    started: Instant,
) {
    let status_str = match resp.status {
        TriageStatus::FollowUp => "follow_up",
        TriageStatus::Completed if resp.is_fallback => "fallback",
        TriageStatus::Completed => "completed",
        TriageStatus::Emergency => "emergency",
    };
    let log = TriageAuditLog {
        id: Uuid::new_v4(),
        session_id: req.session_id.clone(),
        patient_id: req.patient_id.clone(),
        tenant_id: Some(req.tenant_id.clone()),
        red_flag: resp.red_flag,
        triage_status: status_str.to_string(),
        recommended_department: resp
            .triage_result
            .as_ref()
            .map(|c| c.primary_department.clone()),
        urgency_level: resp.triage_result.as_ref().map(|c| c.urgency_level),
        provider_id,
        model_name,
        prompt_tokens,
        completion_tokens,
        latency_ms: started.elapsed().as_millis() as u64,
        seasonal_contexts_applied: resp.seasonal_context_applied.clone(),
        citation_doc_ids: resp.citations.iter().map(|c| c.doc_id.clone()).collect(),
        error_message: None,
        created_at: Utc::now(),
    };
    let _ = res.clients.stores.audit_logs.append(&log).await;
}

/// Ensure `TriageEngine` and its async `run_triage` future are `Send + Sync` for Axum.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TriageEngine>();
};
