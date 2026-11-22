<script setup lang="ts">
import { computed, nextTick, onActivated, onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';

const toast = useToast();

const patients = ref<string[]>([]);
const selectedPatient = ref('');
const sessionId = ref('studio_debug_session_01');
const regionCode = ref('CN-110000');
const population = ref<'adult' | 'child' | 'elderly' | 'pregnant'>('adult');
const utterance = ref('');
const isSending = ref(false);
const showRawJson = ref(false);
const chatContainerRef = ref<HTMLDivElement | null>(null);
const runtimeStatus = ref<{
  using_real_llm: boolean;
  active_providers: string[];
  knowledge_doc_count: number;
  patient_count: number;
  epidemic_count: number;
  synonym_count: number;
  red_flag_count: number;
} | null>(null);

const messages = ref<
  { role: string; text: string; options?: string[]; card?: any; redFlag?: string }[]
>([]);
const latestTrace = ref<any>(null);

const quickScenarios = [
  { label: '急诊120胸痛', pop: 'elderly' as const, text: '老人突然心口窝撕裂样剧烈胸痛，伴随大汗和左臂放射痛' },
  { label: '冬春甲流高热', pop: 'adult' as const, text: '高热39度伴全身肌肉酸痛、咽痛干咳两天' },
  { label: '儿童夜间干咳', pop: 'child' as const, text: '5岁孩子发烧38.5度，刺激性干咳夜间加重三天' },
  { label: '急性胃肠炎', pop: 'adult' as const, text: '昨晚吃海鲜后突发恶心呕吐、腹痛、水样便四次' },
];

const maxStageMs = computed(() => {
  if (!latestTrace.value?.stage_timings?.length) return 1;
  return Math.max(
    1,
    ...latestTrace.value.stage_timings.map((s: any) => Number(s.duration_ms) || 0),
  );
});

async function scrollChatToBottom() {
  await nextTick();
  if (chatContainerRef.value) {
    chatContainerRef.value.scrollTop = chatContainerRef.value.scrollHeight;
  }
}

async function init() {
  try {
    const [pts, st] = await Promise.all([
      invokeStudio<string[]>('list_debug_patients'),
      invokeStudio<any>('get_ecs_runtime_status'),
    ]);
    patients.value = pts ?? [];
    if (!selectedPatient.value || !patients.value.includes(selectedPatient.value)) {
      selectedPatient.value = patients.value[0] ?? 'pat_sha256_demo_001';
    }
    runtimeStatus.value = st;
  } catch (err) {
    toast.error(`加载调试状态与患者列表失败：${String(err)}`);
  }
}

async function sendUtterance(text?: string) {
  const input = (text ?? utterance.value).trim();
  if (!input || isSending.value) return;

  isSending.value = true;
  messages.value.push({ role: 'patient', text: input });
  utterance.value = '';
  await scrollChatToBottom();

  try {
    const res = await invokeStudio<any>('run_ecs_whitebox_triage', {
      req: {
        session_id: sessionId.value,
        patient_id: selectedPatient.value,
        utterance: input,
        region_code: regionCode.value,
        target_population: [population.value],
      },
    });
    latestTrace.value = res.trace;
    const r = res.response;
    messages.value.push({
      role: 'assistant',
      text: r.question || r.triage_result?.summary || '检测到急危重症，请立即拨打 120！',
      options: r.quick_options,
      card: r.triage_result,
      redFlag: r.red_flag,
    });
    await scrollChatToBottom();
  } catch (err) {
    toast.error(`ECS 分诊执行失败：${String(err)}`);
  } finally {
    isSending.value = false;
  }
}

async function runScenario(sc: (typeof quickScenarios)[number]) {
  population.value = sc.pop;
  await resetSession(true);
  await sendUtterance(sc.text);
}

async function resetSession(silent = false) {
  try {
    await invokeStudio('reset_ecs_debug_session', { sessionId: sessionId.value });
    messages.value = [];
    latestTrace.value = null;
    if (!silent) toast.info('已重置当前 ECS 调试会话');
  } catch (err) {
    toast.error(`重置会话失败：${String(err)}`);
  }
}

onMounted(init);
onActivated(init);
</script>

<template>
  <section class="space-y-4">
    <header class="flex flex-wrap items-start justify-between gap-3">
      <div>
        <h2 class="text-xl font-semibold text-white">ECS 分诊白盒调试台（Sandbox）</h2>
        <p class="text-sm text-slate-400 mt-0.5">左侧模拟小程序多轮问诊，右侧实时透视 bevy_ecs 各阶段耗时瀑布图与 Component 状态</p>
      </div>
      <div class="flex flex-wrap gap-2 items-center text-xs">
        <select v-model="population" class="px-2.5 py-1.5 rounded-lg bg-slate-900 border border-slate-700 text-slate-200">
          <option value="adult">成人 (adult)</option>
          <option value="child">儿童 (child)</option>
          <option value="elderly">老人 (elderly)</option>
          <option value="pregnant">孕妇 (pregnant)</option>
        </select>
        <input
          v-model="regionCode"
          placeholder="地区码"
          class="w-28 px-2.5 py-1.5 rounded-lg bg-slate-900 border border-slate-700 font-mono text-slate-200"
        />
        <select v-model="selectedPatient" class="px-2.5 py-1.5 rounded-lg bg-slate-900 border border-slate-700 font-mono text-slate-200 max-w-[180px]">
          <option v-for="p in patients" :key="p" :value="p">{{ p.slice(0, 22) }}...</option>
        </select>
        <button
          @click="init"
          class="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200"
        >
          刷新数据源
        </button>
        <button
          @click="resetSession(false)"
          class="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200"
        >
          重置会话
        </button>
      </div>
    </header>

    <!-- Runtime Data Linkage Status Bar -->
    <div
      v-if="runtimeStatus"
      class="flex flex-wrap items-center justify-between gap-2 px-3.5 py-2.5 rounded-xl bg-slate-900 border border-slate-800 text-xs"
    >
      <div class="flex flex-wrap items-center gap-2">
        <span class="text-slate-400 font-medium">当前分诊联动数据源：</span>
        <span
          :class="runtimeStatus.knowledge_doc_count > 0 ? 'bg-sky-950/80 text-sky-300 border-sky-700/60' : 'bg-slate-800 text-slate-400 border-slate-700'"
          class="px-2 py-0.5 rounded-full border"
        >
          知识库 {{ runtimeStatus.knowledge_doc_count }} 条
        </span>
        <span class="px-2 py-0.5 rounded-full bg-emerald-950/80 text-emerald-300 border border-emerald-700/60">
          病例库 {{ runtimeStatus.patient_count }} 份
        </span>
        <span class="px-2 py-0.5 rounded-full bg-amber-950/80 text-amber-300 border border-amber-700/60">
          流行病 {{ runtimeStatus.epidemic_count }} 条
        </span>
      </div>
      <div class="flex items-center gap-2">
        <span
          :class="
            runtimeStatus.using_real_llm
              ? 'bg-emerald-950 text-emerald-300 border-emerald-600'
              : 'bg-amber-950 text-amber-300 border-amber-700'
          "
          class="px-2.5 py-0.5 rounded-full border font-medium"
        >
          {{
            runtimeStatus.using_real_llm
              ? `真实 AI 对话引擎：${runtimeStatus.active_providers.join(' / ')}`
              : '离线 Mock 模拟模式（在模块 6 配置真实 API Key 后自动切换真实 AI 对话）'
          }}
        </span>
      </div>
    </div>

    <!-- Quick Test Scenario Chips -->
    <div class="flex flex-wrap items-center gap-2 text-xs">
      <span class="text-slate-400">快捷测试场景：</span>
      <button
        v-for="sc in quickScenarios"
        :key="sc.label"
        :disabled="isSending"
        @click="runScenario(sc)"
        class="px-2.5 py-1 rounded-full bg-slate-900 hover:bg-sky-900/60 border border-slate-700 hover:border-sky-600 text-slate-200 transition-colors disabled:opacity-50"
      >
        {{ sc.label }}
      </button>
    </div>

    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <!-- Left: Simulated MiniApp Chat -->
      <div class="flex flex-col h-[calc(100vh-210px)] min-h-[460px] rounded-xl bg-slate-900 border border-slate-800 p-4">
        <div ref="chatContainerRef" class="flex-1 overflow-y-auto space-y-3 pr-1">
          <div v-if="messages.length === 0" class="h-full flex flex-col items-center justify-center text-slate-500 text-xs space-y-2">
            <p>输入患者主诉，或点击上方「快捷测试场景」开始白盒分诊调试</p>
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
              class="px-3.5 py-2.5 rounded-2xl text-sm max-w-[85%] text-left leading-relaxed"
            >
              {{ m.text }}
            </div>
            <div v-if="m.options && m.options.length" class="mt-2 flex flex-wrap gap-1.5">
              <button
                v-for="opt in m.options"
                :key="opt"
                :disabled="isSending"
                @click="sendUtterance(opt)"
                class="px-2.5 py-1 rounded-full bg-slate-800 hover:bg-sky-700 border border-sky-600/50 text-xs text-sky-200 transition-colors"
              >
                {{ opt }}
              </button>
            </div>
            <div v-if="m.card" class="mt-2 p-3 rounded-xl bg-emerald-950/60 border border-emerald-700 text-xs text-left space-y-1.5">
              <div class="flex items-center justify-between">
                <span class="font-semibold text-emerald-300 text-sm">推荐科室：{{ m.card.primary_department }}</span>
                <span class="px-2 py-0.5 rounded-full bg-amber-950/80 text-amber-300 border border-amber-700/60">
                  紧急度 L{{ m.card.urgency_level }}
                </span>
              </div>
              <div class="text-slate-200 leading-relaxed">{{ m.card.summary }}</div>
              <div class="text-slate-400 text-[11px]">{{ m.card.disclaimer }}</div>
            </div>
          </div>
          <div v-if="isSending" class="text-left">
            <span class="inline-block px-3 py-2 rounded-xl bg-slate-800 text-slate-400 text-xs animate-pulse">
              ECS 分诊引擎推理中...
            </span>
          </div>
        </div>
        <div class="mt-3 pt-3 border-t border-slate-800 flex gap-2">
          <input
            v-model="utterance"
            @keyup.enter="sendUtterance()"
            placeholder="输入患者主诉，按 Enter 发送..."
            class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm text-slate-100"
          />
          <button
            :disabled="isSending"
            @click="sendUtterance()"
            class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-sm font-medium text-white"
          >
            {{ isSending ? '执行中' : '发送' }}
          </button>
        </div>
      </div>

      <!-- Right: ECS Component Inspector -->
      <div class="h-[calc(100vh-210px)] min-h-[460px] overflow-y-auto rounded-xl bg-slate-900 border border-slate-800 p-4 space-y-3 text-xs">
        <div class="flex items-center justify-between">
          <h3 class="font-semibold text-emerald-300 text-sm">ECS Inspector（实时 Component 透视）</h3>
          <button
            v-if="latestTrace"
            @click="showRawJson = !showRawJson"
            class="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 text-[11px] text-slate-300"
          >
            {{ showRawJson ? '切换结构化视图' : '查看原始 JSON' }}
          </button>
        </div>

        <div v-if="!latestTrace" class="h-64 flex items-center justify-center text-slate-500">
          发送任意消息以查看 ECS 执行轨迹与各阶段耗时...
        </div>

        <template v-else>
          <!-- Stage Waterfall -->
          <div class="p-3 rounded-lg bg-slate-950 border border-slate-800 space-y-2">
            <div class="flex items-center justify-between text-slate-300">
              <span class="font-semibold">Stage Waterfall</span>
              <span class="font-mono text-sky-300">
                总耗时 {{ latestTrace.total_duration_ms }}ms · LLM {{ latestTrace.llm_calls_made }} 次
              </span>
            </div>
            <div class="space-y-1.5">
              <div v-for="st in latestTrace.stage_timings" :key="st.stage" class="space-y-0.5">
                <div class="flex justify-between text-[11px]">
                  <span :class="st.skipped ? 'text-slate-500 line-through' : 'text-slate-300'">{{ st.stage }}</span>
                  <span class="font-mono text-slate-400">{{ st.skipped ? 'skipped' : `${st.duration_ms} ms` }}</span>
                </div>
                <div class="h-1.5 rounded-full bg-slate-800 overflow-hidden">
                  <div
                    :style="{ width: st.skipped ? '0%' : `${Math.max(4, (st.duration_ms / maxStageMs) * 100)}%` }"
                    class="h-full rounded-full bg-gradient-to-r from-sky-500 to-teal-400 transition-all"
                  />
                </div>
              </div>
            </div>
          </div>

          <template v-if="!showRawJson">
            <!-- Structured Symptoms & RedFlag -->
            <div class="p-3 rounded-lg bg-slate-950 border border-slate-800 space-y-2">
              <div class="flex items-center justify-between">
                <span class="text-sky-300 font-semibold">ExtractedSymptoms & RedFlagAlert</span>
                <span
                  :class="
                    latestTrace.red_flag_alert?.level === 'Emergency120'
                      ? 'bg-rose-950 text-rose-300 border-rose-700'
                      : 'bg-emerald-950 text-emerald-300 border-emerald-700'
                  "
                  class="px-2 py-0.5 rounded-full border text-[11px]"
                >
                  {{ latestTrace.red_flag_alert?.level || 'None' }}
                </span>
              </div>
              <div class="flex flex-wrap gap-1.5">
                <span
                  v-for="sym in latestTrace.extracted_symptoms?.normalized_terms || []"
                  :key="sym"
                  class="px-2 py-0.5 rounded-full bg-indigo-950/80 text-indigo-200 border border-indigo-700/60"
                >
                  {{ sym }}
                </span>
                <span v-if="!latestTrace.extracted_symptoms?.normalized_terms?.length" class="text-slate-500">未命中标准症状词</span>
              </div>
              <div v-if="latestTrace.red_flag_alert?.alert_message" class="text-rose-300">
                {{ latestTrace.red_flag_alert.alert_message }}
              </div>
            </div>

            <!-- Structured Slots & RAG -->
            <div class="p-3 rounded-lg bg-slate-950 border border-slate-800 space-y-2">
              <div class="text-amber-300 font-semibold">TriageSlots & RAG Evidence（知识库召回）</div>
              <div class="grid grid-cols-2 gap-2 text-[11px]">
                <div><span class="text-slate-400">轮次：</span>{{ latestTrace.triage_slots?.turn }} / {{ latestTrace.triage_slots?.max_turns }}</div>
                <div><span class="text-slate-400">RAG 命中：</span>{{ latestTrace.rag_evidence?.items?.length ?? 0 }} 条</div>
                <div><span class="text-slate-400">主诉：</span>{{ latestTrace.triage_slots?.chief_complaint || '-' }}</div>
                <div><span class="text-slate-400">病程：</span>{{ latestTrace.triage_slots?.duration || '-' }}</div>
                <div><span class="text-slate-400">体温/生命体征：</span>{{ latestTrace.triage_slots?.temperature_or_vitals || '-' }}</div>
                <div><span class="text-slate-400">流行病加权：</span>{{ (latestTrace.rag_evidence?.seasonal_context_applied || []).join('、') || '无' }}</div>
              </div>
              <div v-if="latestTrace.session_memory?.rewritten_query" class="text-[11px] text-slate-400 pt-1 border-t border-slate-800">
                <span class="text-slate-500">检索改写 Query：</span>{{ latestTrace.session_memory.rewritten_query }}
              </div>
              <div v-if="latestTrace.rag_evidence?.items?.length" class="space-y-1.5 pt-1 border-t border-slate-800">
                <div
                  v-for="item in latestTrace.rag_evidence.items.slice(0, 3)"
                  :key="item.doc_id"
                  class="p-2 rounded bg-slate-900 border border-slate-800 text-[11px] space-y-0.5"
                >
                  <div class="flex items-center justify-between gap-2">
                    <span class="font-semibold text-sky-300">{{ item.disease_name }} · {{ item.department }}</span>
                    <span class="font-mono text-emerald-300">得分 {{ Number(item.final_score).toFixed(3) }}</span>
                  </div>
                  <p class="text-slate-400 line-clamp-2">{{ item.snippet }}</p>
                </div>
              </div>
              <div v-else class="text-[11px] text-slate-500 pt-1 border-t border-slate-800">
                本次未命中知识库条目（请确认已在模块 1 导入对应医学知识）
              </div>
            </div>

            <!-- Patient Case Memory (L2 + L3) -->
            <div class="p-3 rounded-lg bg-slate-950 border border-slate-800 space-y-2">
              <div class="flex items-center justify-between">
                <span class="text-emerald-300 font-semibold">Patient Memory（L2 画像 & L3 历史病例）</span>
                <span class="text-[11px] text-slate-400">
                  L3 召回 {{ latestTrace.memory_recall?.l3_events?.length ?? 0 }} 条
                </span>
              </div>
              <div v-if="latestTrace.patient_profile?.l2_profile" class="grid grid-cols-2 gap-1.5 text-[11px]">
                <div><span class="text-slate-400">年龄段/性别：</span>{{ latestTrace.patient_profile.l2_profile.age_group }} / {{ latestTrace.patient_profile.l2_profile.gender }}</div>
                <div><span class="text-slate-400">慢病：</span>{{ (latestTrace.patient_profile.l2_profile.chronic_diseases || []).join('、') || '无' }}</div>
                <div><span class="text-slate-400">过敏史：</span>{{ (latestTrace.patient_profile.l2_profile.allergies || []).join('、') || '无' }}</div>
                <div><span class="text-slate-400">长期用药：</span>{{ (latestTrace.patient_profile.l2_profile.long_term_medications || []).join('、') || '无' }}</div>
              </div>
              <div v-else class="text-[11px] text-slate-500">当前患者无 L2 长期健康画像</div>

              <div v-if="latestTrace.memory_recall?.l3_events?.length" class="space-y-1 pt-1 border-t border-slate-800">
                <div
                  v-for="ev in latestTrace.memory_recall.l3_events"
                  :key="ev.event.event_id"
                  class="p-2 rounded bg-slate-900 border border-slate-800 text-[11px]"
                >
                  <div class="flex items-center justify-between text-emerald-300">
                    <span>历史主诉：{{ ev.event.chief_complaint }}</span>
                    <span class="font-mono text-slate-400">{{ ev.event.recommended_department }} · L{{ ev.event.urgency_level }}</span>
                  </div>
                  <div class="text-slate-400 mt-0.5">{{ ev.event.summary }}</div>
                </div>
              </div>
            </div>
          </template>

          <template v-else>
            <div class="p-2.5 rounded bg-slate-950 border border-slate-800">
              <div class="text-sky-300 font-semibold mb-1">ExtractedSymptoms & RedFlagAlert</div>
              <pre class="whitespace-pre-wrap text-slate-300 font-mono text-[11px]">{{ JSON.stringify({ extracted_symptoms: latestTrace.extracted_symptoms, red_flag_alert: latestTrace.red_flag_alert }, null, 2) }}</pre>
            </div>
            <div class="p-2.5 rounded bg-slate-950 border border-slate-800">
              <div class="text-amber-300 font-semibold mb-1">TriageSlots & RagEvidence</div>
              <pre class="whitespace-pre-wrap text-slate-300 font-mono text-[11px]">{{ JSON.stringify({ triage_slots: latestTrace.triage_slots, rag_evidence: latestTrace.rag_evidence }, null, 2) }}</pre>
            </div>
          </template>
        </template>
      </div>
    </div>
  </section>
</template>
