<script setup lang="ts">
import { computed, nextTick, onActivated, onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';

const toast = useToast();

interface PatientSummary {
  patient_id: string;
  age_group: string;
  gender: string;
  chronic_diseases: string[];
  visit_count: number;
  latest_complaint: string | null;
  latest_department: string | null;
  latest_visit_at: string | null;
}

const mode = ref<'revisit' | 'new_patient'>('revisit');
const salt = ref('');

// Patient lookup & serial state
const patientSummaries = ref<PatientSummary[]>([]);
const serialInput = ref('');
const resolvedPatientId = ref('');
const currentRecord = ref<any>(null);
const lookupNotFound = ref(false);

// Editable L2 profile (live-editable during consultation)
const l2AgeGroup = ref('30-39');
const l2Gender = ref<'male' | 'female' | 'other' | 'unknown'>('unknown');
const l2Pregnant = ref(false);
const l2ChronicText = ref('');
const l2AllergyText = ref('');
const l2SurgicalText = ref('');
const l2MedicationText = ref('');
const l2Dirty = ref(false);
const syncL2OnSend = ref(true);

// Revisit feedback form
const outcomeOptions = ['明显好转', '部分好转', '症状持续', '症状加重', '出现新症状', '初诊建档'];
const fbOutcome = ref('部分好转');
const fbChiefComplaint = ref('');
const fbDepartment = ref('全科医学科');
const fbUrgency = ref(3);
const fbSymptomsText = ref('');
const fbSummary = ref('');

// Consultation chat state
const sessionId = ref(`consult_${Date.now()}`);
const population = ref<'adult' | 'child' | 'elderly' | 'pregnant'>('adult');
const regionCode = ref('CN-110000');
const utterance = ref('');
const isSending = ref(false);
const isSaving = ref(false);
const chatRef = ref<HTMLDivElement | null>(null);
const messages = ref<
  {
    role: 'patient' | 'assistant';
    text: string;
    options?: string[];
    card?: any;
    redFlag?: string;
    memoryUpdated?: boolean;
    ragHits?: number;
    l3Recalled?: number;
  }[]
>([]);

function generateNewSerial(): string {
  const now = new Date();
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, '0');
  const d = String(now.getDate()).padStart(2, '0');
  const rand = Math.random().toString(36).slice(2, 6).toUpperCase();
  return `PAT-${y}${m}${d}-${rand}`;
}

function splitTags(text: string): string[] {
  return text
    .split(/[,，、;；\n]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function buildL2Payload() {
  return {
    age_group: l2AgeGroup.value.trim() || 'unknown',
    gender: l2Gender.value,
    is_pregnant: l2Pregnant.value,
    chronic_diseases: splitTags(l2ChronicText.value),
    allergies: splitTags(l2AllergyText.value),
    surgical_history: splitTags(l2SurgicalText.value),
    long_term_medications: splitTags(l2MedicationText.value),
  };
}

function hydrateL2FromRecord(rec: any) {
  if (!rec?.l2_profile) {
    l2AgeGroup.value = '30-39';
    l2Gender.value = 'unknown';
    l2Pregnant.value = false;
    l2ChronicText.value = '';
    l2AllergyText.value = '';
    l2SurgicalText.value = '';
    l2MedicationText.value = '';
    l2Dirty.value = false;
    return;
  }
  const l2 = rec.l2_profile;
  l2AgeGroup.value = l2.age_group || 'unknown';
  l2Gender.value = l2.gender || 'unknown';
  l2Pregnant.value = Boolean(l2.is_pregnant);
  l2ChronicText.value = (l2.chronic_diseases || []).join('、');
  l2AllergyText.value = (l2.allergies || []).join('、');
  l2SurgicalText.value = (l2.surgical_history || []).join('、');
  l2MedicationText.value = (l2.long_term_medications || []).join('、');
  l2Dirty.value = false;
}

function resetL2FormForNewPatient() {
  l2AgeGroup.value = '30-39';
  l2Gender.value = 'unknown';
  l2Pregnant.value = false;
  l2ChronicText.value = '';
  l2AllergyText.value = '';
  l2SurgicalText.value = '';
  l2MedicationText.value = '';
  l2Dirty.value = false;
}

const activeSerialReady = computed(() => serialInput.value.trim().length > 0);

async function scrollChat() {
  await nextTick();
  if (chatRef.value) {
    chatRef.value.scrollTop = chatRef.value.scrollHeight;
  }
}

async function loadPatientList() {
  try {
    patientSummaries.value = await invokeStudio<PatientSummary[]>('list_patient_summaries', {
      limit: 200,
    });
  } catch (err) {
    toast.error(`加载病人列表失败：${String(err)}`);
  }
}

async function handleLookupSerial(customSerial?: string) {
  const target = (customSerial ?? serialInput.value).trim();
  if (!target) {
    toast.error('请输入病人序列号或选择已有病人');
    return;
  }
  serialInput.value = target;
  lookupNotFound.value = false;
  try {
    const res = await invokeStudio<any>('lookup_patient_by_serial', {
      serialOrId: target,
      salt: salt.value,
    });
    resolvedPatientId.value = res.resolved_patient_id;
    if (res.found && res.record) {
      currentRecord.value = res.record;
      hydrateL2FromRecord(res.record);
      const latest = res.record.l3_visit_events?.[0];
      if (latest) {
        fbDepartment.value = latest.recommended_department || '全科医学科';
      }
      resetChatOnly();
      toast.success(
        `已定位病人档案（历史就诊 ${res.record.l3_visit_events?.length ?? 0} 次），复诊问诊将自动携带完整记忆`,
      );
    } else {
      currentRecord.value = null;
      resetL2FormForNewPatient();
      lookupNotFound.value = true;
      resetChatOnly();
      toast.info('未查到该序列号的历史病历，可点击「切换为新病人建档」开始无记忆初诊');
    }
  } catch (err) {
    toast.error(`查询病人序列号失败：${String(err)}`);
  }
}

function switchToRevisitMode() {
  mode.value = 'revisit';
  fbOutcome.value = '部分好转';
  lookupNotFound.value = false;
  resetChatOnly();
}

function switchToNewPatientMode(keepSerial = false) {
  mode.value = 'new_patient';
  fbOutcome.value = '初诊建档';
  lookupNotFound.value = false;
  currentRecord.value = null;
  resolvedPatientId.value = '';
  if (!keepSerial || !serialInput.value.trim()) {
    serialInput.value = generateNewSerial();
  }
  resetL2FormForNewPatient();
  resetChatOnly();
  toast.info(`已切换新病人建档模式（序列号：${serialInput.value}），不携带任何历史记忆`);
}

function resetChatOnly() {
  sessionId.value = `consult_${Date.now()}_${Math.random().toString(36).slice(2, 5)}`;
  messages.value = [];
}

async function copyText(text: string, label: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(`已复制${label}`);
  } catch {
    toast.error('复制失败');
  }
}

async function handleSaveL2Profile() {
  if (!activeSerialReady.value) {
    toast.error('请先输入或生成病人序列号');
    return;
  }
  isSaving.value = true;
  try {
    const rec = await invokeStudio<any>('create_or_update_patient_profile', {
      req: {
        serial_or_id: serialInput.value.trim(),
        salt: salt.value,
        consent_long_term_memory: true,
        l2_profile: buildL2Payload(),
      },
    });
    currentRecord.value = rec;
    resolvedPatientId.value = rec.patient_id;
    lookupNotFound.value = false;
    l2Dirty.value = false;
    await loadPatientList();
    toast.success('已保存/更新病人 L2 健康画像，后续问诊将立即生效');
  } catch (err) {
    toast.error(`保存 L2 画像失败：${String(err)}`);
  } finally {
    isSaving.value = false;
  }
}

async function handleSubmitRevisitFeedback() {
  if (!activeSerialReady.value) {
    toast.error('请先输入或选择病人序列号');
    return;
  }
  if (!fbChiefComplaint.value.trim()) {
    toast.error('请填写本次复诊/反馈主诉');
    return;
  }
  isSaving.value = true;
  try {
    const res = await invokeStudio<any>('submit_patient_revisit_feedback', {
      req: {
        serial_or_id: serialInput.value.trim(),
        salt: salt.value,
        outcome_status: fbOutcome.value,
        chief_complaint: fbChiefComplaint.value.trim(),
        extracted_symptoms: splitTags(fbSymptomsText.value),
        actual_department: fbDepartment.value.trim() || '全科医学科',
        urgency_level: Number(fbUrgency.value) || 3,
        feedback_summary: fbSummary.value.trim(),
        updated_l2_profile: l2Dirty.value || mode.value === 'new_patient' ? buildL2Payload() : null,
      },
    });
    currentRecord.value = res.patient_record;
    resolvedPatientId.value = res.resolved_patient_id;
    lookupNotFound.value = false;
    l2Dirty.value = false;
    fbChiefComplaint.value = '';
    fbSymptomsText.value = '';
    fbSummary.value = '';
    await loadPatientList();
    toast.success('复诊反馈已写入 L3 病历时间轴并同步向量库，下次复诊可自动召回');
  } catch (err) {
    toast.error(`提交复诊反馈失败：${String(err)}`);
  } finally {
    isSaving.value = false;
  }
}

async function sendConsultation(text?: string) {
  const input = (text ?? utterance.value).trim();
  if (!input || isSending.value) return;
  if (!activeSerialReady.value) {
    toast.error('请先输入病人序列号（或点击「新病人初诊建档」自动生成）');
    return;
  }

  isSending.value = true;
  messages.value.push({ role: 'patient', text: input });
  utterance.value = '';
  await scrollChat();

  try {
    const shouldPushL2 = syncL2OnSend.value && l2Dirty.value;
    const res = await invokeStudio<any>('run_consultation_triage', {
      req: {
        session_id: sessionId.value,
        serial_or_id: serialInput.value.trim(),
        salt: salt.value,
        is_new_patient: mode.value === 'new_patient',
        utterance: input,
        region_code: regionCode.value,
        target_population: [population.value],
        live_l2_update: shouldPushL2 ? buildL2Payload() : null,
      },
    });

    resolvedPatientId.value = res.resolved_patient_id;
    if (shouldPushL2) {
      l2Dirty.value = false;
    }
    if (res.updated_patient_record) {
      currentRecord.value = res.updated_patient_record;
      lookupNotFound.value = false;
      if (!l2Dirty.value) {
        hydrateL2FromRecord(res.updated_patient_record);
      }
    }

    const r = res.triage.response;
    const trace = res.triage.trace;
    messages.value.push({
      role: 'assistant',
      text: r.question || r.triage_result?.summary || '检测到急危重症，请立即拨打 120！',
      options: r.quick_options,
      card: r.triage_result,
      redFlag: r.red_flag,
      memoryUpdated: Boolean(res.memory_updated),
      ragHits: trace?.rag_evidence?.items?.length ?? 0,
      l3Recalled: trace?.memory_recall?.l3_events?.length ?? 0,
    });

    if (res.memory_updated) {
      await loadPatientList();
    }
    await scrollChat();
  } catch (err) {
    toast.error(`问诊执行失败：${String(err)}`);
  } finally {
    isSending.value = false;
  }
}

function prefillFeedbackFromCard(card: any) {
  if (!card) return;
  const lastPatientMsg = [...messages.value].reverse().find((m) => m.role === 'patient');
  fbChiefComplaint.value = lastPatientMsg?.text || fbChiefComplaint.value;
  fbDepartment.value = card.primary_department || fbDepartment.value;
  fbUrgency.value = card.urgency_level || 3;
  fbSummary.value = card.summary || '';
  toast.info('已将本次分诊结论填入右侧复诊反馈表单，补充实际转归后可提交保存');
}

onMounted(async () => {
  await loadPatientList();
  if (patientSummaries.value.length > 0 && !serialInput.value) {
    await handleLookupSerial(patientSummaries.value[0].patient_id);
  }
});

onActivated(loadPatientList);
</script>

<template>
  <section class="space-y-4">
    <!-- Header -->
    <header class="flex flex-wrap items-center justify-between gap-3">
      <div>
        <h2 class="text-xl font-semibold text-white">病人复诊反馈与新病人建档问诊</h2>
        <p class="text-sm text-slate-400">
          按病人序列号调取历史记忆进行复诊反馈；新病人不带旧记忆直接初诊建档，问诊过程中可实时更新 L2/L3 病历数据
        </p>
      </div>
      <div class="flex rounded-lg bg-slate-900 p-1 border border-slate-800 text-xs">
        <button
          @click="switchToRevisitMode"
          :class="mode === 'revisit' ? 'bg-sky-600 text-white' : 'text-slate-400 hover:text-white'"
          class="px-3.5 py-1.5 rounded-md font-medium transition-colors"
        >
          老病人复诊反馈（带记忆）
        </button>
        <button
          @click="switchToNewPatientMode(false)"
          :class="mode === 'new_patient' ? 'bg-emerald-600 text-white' : 'text-slate-400 hover:text-white'"
          class="px-3.5 py-1.5 rounded-md font-medium transition-colors"
        >
          + 新病人初诊建档（无旧记忆）
        </button>
      </div>
    </header>

    <!-- Patient Serial Lookup / Creation Bar -->
    <div class="bg-slate-900 p-4 rounded-xl border border-slate-800 space-y-3">
      <div class="flex flex-wrap items-center gap-2">
        <div class="flex-1 min-w-[240px]">
          <label class="block text-xs text-slate-400 mb-1">
            {{ mode === 'revisit' ? '输入病人序列号 / 匿名 Patient ID 查询复诊档案' : '新病人序列号（自动生成或手工输入院内病历号）' }}
          </label>
          <div class="flex gap-2">
            <input
              v-model="serialInput"
              @keyup.enter="mode === 'revisit' && handleLookupSerial()"
              placeholder="例如：PAT-20261122-A1B2 或 pat_sha256_..."
              class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm font-mono text-slate-100"
            />
            <button
              v-if="mode === 'revisit'"
              @click="handleLookupSerial()"
              class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium text-white shrink-0"
            >
              按序列号查询
            </button>
            <button
              v-else
              @click="serialInput = generateNewSerial(); currentRecord = null; resolvedPatientId = ''; resetChatOnly()"
              class="px-3.5 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs text-emerald-300 shrink-0"
            >
              重新生成序列号
            </button>
          </div>
        </div>

        <div v-if="mode === 'revisit'" class="min-w-[260px]">
          <label class="block text-xs text-slate-400 mb-1">快速选择已有病历（共 {{ patientSummaries.length }} 位）</label>
          <select
            @change="handleLookupSerial(($event.target as HTMLSelectElement).value)"
            :value="currentRecord?.patient_id || ''"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-xs text-slate-200"
          >
            <option value="" disabled>-- 选择已有病人档案 --</option>
            <option v-for="p in patientSummaries" :key="p.patient_id" :value="p.patient_id">
              {{ p.patient_id.slice(0, 18) }}... | {{ p.age_group }}/{{ p.gender }} | {{ p.visit_count }}次就诊 {{ p.latest_complaint ? `(${p.latest_complaint.slice(0, 12)})` : '' }}
            </option>
          </select>
        </div>
      </div>

      <!-- Status line -->
      <div class="flex flex-wrap items-center justify-between gap-2 text-xs pt-1 border-t border-slate-800">
        <div class="flex flex-wrap items-center gap-2">
          <span
            v-if="mode === 'new_patient' && !currentRecord"
            class="px-2.5 py-0.5 rounded-full bg-emerald-950 text-emerald-300 border border-emerald-700"
          >
            新病人无记忆状态：不会召回任何历史病历，问诊或保存后将新建档案
          </span>
          <span
            v-else-if="currentRecord"
            class="px-2.5 py-0.5 rounded-full bg-sky-950 text-sky-300 border border-sky-700"
          >
            已加载病历档案：L2 画像 + {{ currentRecord.l3_visit_events?.length ?? 0 }} 条 L3 历史就诊记录
          </span>
          <span
            v-else-if="lookupNotFound"
            class="px-2.5 py-0.5 rounded-full bg-amber-950 text-amber-300 border border-amber-700 flex items-center gap-2"
          >
            <span>未查到该序列号的历史记录</span>
            <button @click="switchToNewPatientMode(true)" class="underline text-emerald-300 font-medium">
              直接以此序列号新建病人
            </button>
          </span>
        </div>

        <div v-if="resolvedPatientId" class="flex items-center gap-2 text-slate-400 font-mono">
          <span>匿名ID: {{ resolvedPatientId.slice(0, 22) }}...</span>
          <button @click="copyText(resolvedPatientId, '匿名 Patient ID')" class="text-sky-400 hover:underline">
            复制ID
          </button>
          <button
            v-if="serialInput && serialInput !== resolvedPatientId"
            @click="copyText(serialInput, '病人序列号')"
            class="text-emerald-400 hover:underline"
          >
            复制序列号
          </button>
        </div>
      </div>
    </div>

    <!-- Main 2-column Workspace -->
    <div class="grid grid-cols-1 lg:grid-cols-12 gap-4">
      <!-- Left (7 cols): Live Consultation Chat -->
      <div class="lg:col-span-7 flex flex-col h-[calc(100vh-275px)] min-h-[500px] rounded-xl bg-slate-900 border border-slate-800 p-4">
        <div class="flex flex-wrap items-center justify-between gap-2 pb-3 border-b border-slate-800 text-xs">
          <div class="font-semibold text-white">
            {{ mode === 'revisit' ? '复诊智能问诊（自动融合知识库 + 历史病例 + AI对话）' : '新病人初诊问诊（无旧记忆，问诊即建档更新）' }}
          </div>
          <div class="flex items-center gap-2">
            <select v-model="population" class="px-2 py-1 rounded bg-slate-950 border border-slate-700 text-slate-200">
              <option value="adult">成人</option>
              <option value="child">儿童</option>
              <option value="elderly">老人</option>
              <option value="pregnant">孕妇</option>
            </select>
            <input v-model="regionCode" class="w-24 px-2 py-1 rounded bg-slate-950 border border-slate-700 font-mono text-slate-200" />
            <button @click="resetChatOnly" class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-slate-300">
              新问诊会话
            </button>
          </div>
        </div>

        <div ref="chatRef" class="flex-1 overflow-y-auto py-3 space-y-3 pr-1">
          <div v-if="messages.length === 0" class="h-full flex flex-col items-center justify-center text-slate-500 text-xs space-y-2 text-center px-6">
            <p v-if="mode === 'revisit'">
              输入本次复诊主诉或疗效反馈（例如：“上次开的布地奈德用了三天，夜间干咳好转但仍有轻微胸闷”），系统将结合该病人历史病历与知识库进行复诊分诊，并在收敛后自动更新病历。
            </p>
            <p v-else>
              新病人序列号 <span class="font-mono text-emerald-300">{{ serialInput }}</span> 已就绪。直接输入主诉即可开始无记忆初诊，右侧可随时补充 L2 画像，分诊完成后自动建档入库。
            </p>
          </div>

          <div v-for="(m, idx) in messages" :key="idx" :class="m.role === 'patient' ? 'text-right' : 'text-left'">
            <div
              :class="
                m.role === 'patient'
                  ? 'inline-block bg-sky-600 text-white'
                  : m.redFlag === 'Emergency120'
                    ? 'inline-block bg-rose-950/90 text-rose-100 border border-rose-700'
                    : 'inline-block bg-slate-800 text-slate-100'
              "
              class="px-3.5 py-2.5 rounded-2xl text-sm max-w-[88%] text-left leading-relaxed"
            >
              {{ m.text }}
            </div>

            <div v-if="m.role === 'assistant'" class="mt-1 flex flex-wrap items-center gap-2 text-[11px] text-slate-400">
              <span>知识库命中 {{ m.ragHits ?? 0 }} 条</span>
              <span>·</span>
              <span>召回历史病例 {{ m.l3Recalled ?? 0 }} 条</span>
              <span v-if="m.memoryUpdated" class="px-2 py-0.5 rounded-full bg-emerald-950 text-emerald-300 border border-emerald-700">
                已实时更新病历档案
              </span>
            </div>

            <div v-if="m.options?.length" class="mt-2 flex flex-wrap gap-1.5">
              <button
                v-for="opt in m.options"
                :key="opt"
                :disabled="isSending"
                @click="sendConsultation(opt)"
                class="px-2.5 py-1 rounded-full bg-slate-800 hover:bg-sky-700 border border-sky-600/50 text-xs text-sky-200 transition-colors"
              >
                {{ opt }}
              </button>
            </div>

            <div v-if="m.card" class="mt-2 p-3 rounded-xl bg-emerald-950/60 border border-emerald-700 text-xs text-left space-y-1.5">
              <div class="flex items-center justify-between gap-2">
                <span class="font-semibold text-emerald-300 text-sm">推荐科室：{{ m.card.primary_department }}</span>
                <div class="flex items-center gap-2">
                  <span class="px-2 py-0.5 rounded-full bg-amber-950/80 text-amber-300 border border-amber-700/60">
                    紧急度 L{{ m.card.urgency_level }}
                  </span>
                  <button
                    @click="prefillFeedbackFromCard(m.card)"
                    class="px-2 py-0.5 rounded bg-sky-700 hover:bg-sky-600 text-white text-[11px]"
                  >
                    填入右侧反馈表
                  </button>
                </div>
              </div>
              <div class="text-slate-200 leading-relaxed">{{ m.card.summary }}</div>
            </div>
          </div>

          <div v-if="isSending" class="text-left">
            <span class="inline-block px-3 py-2 rounded-xl bg-slate-800 text-slate-400 text-xs animate-pulse">
              分诊引擎推理与病历同步中...
            </span>
          </div>
        </div>

        <div class="pt-3 border-t border-slate-800 space-y-2">
          <label class="flex items-center gap-2 text-[11px] text-slate-400 cursor-pointer">
            <input type="checkbox" v-model="syncL2OnSend" class="accent-sky-500" />
            <span>问诊发送时自动同步右侧已修改的 L2 画像信息</span>
            <span v-if="l2Dirty" class="text-amber-300">（右侧 L2 有未保存修改）</span>
          </label>
          <div class="flex gap-2">
            <input
              v-model="utterance"
              @keyup.enter="sendConsultation()"
              :placeholder="mode === 'revisit' ? '输入复诊主诉或疗效反馈，按 Enter 发送...' : '输入新病人主诉，按 Enter 开始无记忆初诊...'"
              class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm text-slate-100"
            />
            <button
              :disabled="isSending"
              @click="sendConsultation()"
              class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-sm font-medium text-white"
            >
              {{ isSending ? '问诊中' : '发送问诊' }}
            </button>
          </div>
        </div>
      </div>

      <!-- Right (5 cols): Live Editable L2 Profile + Revisit Feedback + L3 Timeline -->
      <div class="lg:col-span-5 h-[calc(100vh-275px)] min-h-[500px] overflow-y-auto space-y-3 pr-1">
        <!-- Card 1: Live Editable L2 Profile -->
        <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 text-xs">
          <div class="flex items-center justify-between">
            <h3 class="font-semibold text-emerald-300 text-sm">L2 长期健康画像（问诊中可实时更新）</h3>
            <button
              :disabled="isSaving"
              @click="handleSaveL2Profile"
              class="px-3 py-1 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-white font-medium"
            >
              {{ currentRecord ? '更新 L2 档案' : '新建病人档案' }}
            </button>
          </div>
          <div class="grid grid-cols-3 gap-2">
            <div>
              <label class="block text-slate-400 mb-1">年龄段</label>
              <input v-model="l2AgeGroup" @input="l2Dirty = true" placeholder="如 30-39" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
            </div>
            <div>
              <label class="block text-slate-400 mb-1">性别</label>
              <select v-model="l2Gender" @change="l2Dirty = true" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100">
                <option value="unknown">未知</option>
                <option value="male">男</option>
                <option value="female">女</option>
                <option value="other">其他</option>
              </select>
            </div>
            <div class="flex items-end pb-1.5">
              <label class="flex items-center gap-1.5 text-slate-200 cursor-pointer">
                <input type="checkbox" v-model="l2Pregnant" @change="l2Dirty = true" class="accent-rose-500" />
                <span>妊娠期</span>
              </label>
            </div>
          </div>
          <div class="grid grid-cols-2 gap-2">
            <div>
              <label class="block text-slate-400 mb-1">慢病史（顿号/逗号分隔）</label>
              <input v-model="l2ChronicText" @input="l2Dirty = true" placeholder="高血压、轻度哮喘" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
            </div>
            <div>
              <label class="block text-slate-400 mb-1">过敏史</label>
              <input v-model="l2AllergyText" @input="l2Dirty = true" placeholder="青霉素、花粉" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
            </div>
            <div>
              <label class="block text-slate-400 mb-1">手术史</label>
              <input v-model="l2SurgicalText" @input="l2Dirty = true" placeholder="阑尾切除术" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
            </div>
            <div>
              <label class="block text-slate-400 mb-1">长期用药</label>
              <input v-model="l2MedicationText" @input="l2Dirty = true" placeholder="布地奈德吸入剂" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
            </div>
          </div>
        </div>

        <!-- Card 2: Revisit Feedback Form -->
        <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 text-xs">
          <h3 class="font-semibold text-amber-300 text-sm">复诊反馈 / 就诊结果登记（写入 L3 记忆）</h3>
          <div class="flex flex-wrap gap-1.5">
            <button
              v-for="opt in outcomeOptions"
              :key="opt"
              @click="fbOutcome = opt"
              :class="fbOutcome === opt ? 'bg-amber-600 text-white border-amber-500' : 'bg-slate-950 text-slate-300 border-slate-700'"
              class="px-2.5 py-1 rounded-full border transition-colors"
            >
              {{ opt }}
            </button>
          </div>
          <div class="grid grid-cols-3 gap-2">
            <div class="col-span-2">
              <label class="block text-slate-400 mb-1">实际就诊/推荐科室</label>
              <input v-model="fbDepartment" placeholder="呼吸内科" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
            </div>
            <div>
              <label class="block text-slate-400 mb-1">紧急度 (L1-L4)</label>
              <select v-model.number="fbUrgency" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100">
                <option :value="1">L1 危急</option>
                <option :value="2">L2 急诊</option>
                <option :value="3">L3 常规</option>
                <option :value="4">L4 随访</option>
              </select>
            </div>
          </div>
          <div>
            <label class="block text-slate-400 mb-1">本次复诊主诉（必填）</label>
            <input v-model="fbChiefComplaint" placeholder="如：用药3天后干咳减轻，仍有轻微胸闷" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
          </div>
          <div>
            <label class="block text-slate-400 mb-1">提取症状标签（可选，逗号分隔）</label>
            <input v-model="fbSymptomsText" placeholder="干咳、胸闷" class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
          </div>
          <div>
            <label class="block text-slate-400 mb-1">复诊疗效反馈与医生处置摘要</label>
            <textarea v-model="fbSummary" rows="2" placeholder="记录上次分诊准确性、医生诊断/用药调整、后续随访注意事项..." class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-100" />
          </div>
          <button
            :disabled="isSaving"
            @click="handleSubmitRevisitFeedback"
            class="w-full py-2 rounded-lg bg-amber-600 hover:bg-amber-500 disabled:opacity-50 text-white font-medium"
          >
            {{ isSaving ? '写入中...' : '保存复诊反馈并更新 L3 病历记忆' }}
          </button>
        </div>

        <!-- Card 3: L3 Visit History Timeline -->
        <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-2.5 text-xs">
          <div class="flex items-center justify-between">
            <h3 class="font-semibold text-sky-300 text-sm">L3 历史就诊与复诊时间轴</h3>
            <span class="text-slate-400">共 {{ currentRecord?.l3_visit_events?.length ?? 0 }} 条</span>
          </div>
          <div v-if="!currentRecord?.l3_visit_events?.length" class="text-slate-500 py-2">
            {{ mode === 'new_patient' ? '新病人暂无历史就诊记录，问诊完成或提交反馈后将自动显示在此。' : '该病人暂无 L3 就诊记录。' }}
          </div>
          <div v-else class="space-y-2">
            <div
              v-for="ev in currentRecord.l3_visit_events"
              :key="ev.event_id"
              class="p-2.5 rounded-lg bg-slate-950 border border-slate-800 space-y-1"
            >
              <div class="flex items-center justify-between gap-2">
                <span class="font-semibold text-slate-100">{{ ev.chief_complaint }}</span>
                <span class="px-2 py-0.5 rounded bg-sky-950 text-sky-300 border border-sky-800/60 shrink-0">
                  {{ ev.recommended_department }} · L{{ ev.urgency_level }}
                </span>
              </div>
              <div class="text-slate-400 leading-relaxed">{{ ev.summary }}</div>
              <div class="flex flex-wrap items-center justify-between gap-1 text-[10px] text-slate-500 pt-0.5">
                <div class="flex flex-wrap gap-1">
                  <span v-for="s in ev.extracted_symptoms || []" :key="s" class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-300">
                    {{ s }}
                  </span>
                </div>
                <span class="font-mono">{{ (ev.occurred_at || '').slice(0, 19).replace('T', ' ') }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
