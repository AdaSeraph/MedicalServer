<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';

const patients = ref<string[]>([]);
const selectedPatient = ref('');
const sessionId = ref('studio_debug_session_01');
const regionCode = ref('CN-110000');
const utterance = ref('高热39度伴咳嗽咽痛两天');
const messages = ref<{ role: string; text: string; options?: string[]; card?: any }[]>([]);
const latestTrace = ref<any>(null);

async function init() {
  patients.value = await invokeStudio<string[]>('list_debug_patients');
  selectedPatient.value = patients.value[0] ?? 'pat_sha256_demo_001';
}

async function sendUtterance(text?: string) {
  const input = (text ?? utterance.value).trim();
  if (!input) return;
  messages.value.push({ role: 'patient', text: input });
  const res = await invokeStudio<any>('run_ecs_whitebox_triage', {
    req: {
      session_id: sessionId.value,
      patient_id: selectedPatient.value,
      utterance: input,
      region_code: regionCode.value,
      target_population: ['adult'],
    },
  });
  latestTrace.value = res.trace;
  const r = res.response;
  messages.value.push({
    role: 'assistant',
    text: r.question || r.triage_result?.summary || '检测到急危重症，请立即拨打 120！',
    options: r.quick_options,
    card: r.triage_result,
  });
  utterance.value = '';
}

async function resetSession() {
  await invokeStudio('reset_ecs_debug_session', { sessionId: sessionId.value });
  messages.value = [];
  latestTrace.value = null;
}

onMounted(init);
</script>

<template>
  <section class="space-y-4">
    <header class="flex flex-wrap items-center justify-between gap-2">
      <div>
        <h2 class="text-xl font-semibold text-white">ECS 分诊白盒调试台（Sandbox）</h2>
        <p class="text-sm text-slate-400">左侧模拟微信小程序对话，右侧实时透视 bevy_ecs 各 Component 状态与各阶段耗时</p>
      </div>
      <div class="flex gap-2 items-center text-xs">
        <select v-model="selectedPatient" class="px-2 py-1.5 rounded bg-slate-900 border border-slate-700">
          <option v-for="p in patients" :key="p" :value="p">{{ p.slice(0, 22) }}...</option>
        </select>
        <button @click="resetSession" class="px-3 py-1.5 rounded bg-slate-700 hover:bg-slate-600">重置会话</button>
      </div>
    </header>

    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <!-- Left: Simulated MiniApp Chat -->
      <div class="flex flex-col h-[540px] rounded-xl bg-slate-900 border border-slate-800 p-4">
        <div class="flex-1 overflow-y-auto space-y-3 pr-1">
          <div v-for="(m, idx) in messages" :key="idx" :class="m.role === 'patient' ? 'text-right' : 'text-left'">
            <div :class="m.role === 'patient' ? 'inline-block bg-sky-600 text-white' : 'inline-block bg-slate-800 text-slate-100'" class="px-3 py-2 rounded-xl text-sm max-w-[85%]">
              {{ m.text }}
            </div>
            <div v-if="m.options && m.options.length" class="mt-2 flex flex-wrap gap-1.5">
              <button v-for="opt in m.options" :key="opt" @click="sendUtterance(opt)" class="px-2.5 py-1 rounded-full bg-slate-800 hover:bg-sky-700 border border-sky-600/50 text-xs text-sky-200">
                {{ opt }}
              </button>
            </div>
            <div v-if="m.card" class="mt-2 p-3 rounded-lg bg-emerald-950/60 border border-emerald-700 text-xs text-left space-y-1">
              <div class="font-semibold text-emerald-300">推荐科室：{{ m.card.primary_department }}（紧急度 {{ m.card.urgency_level }}）</div>
              <div class="text-slate-300">{{ m.card.summary }}</div>
              <div class="text-slate-400">{{ m.card.disclaimer }}</div>
            </div>
          </div>
        </div>
        <div class="mt-3 flex gap-2">
          <input v-model="utterance" @keyup.enter="sendUtterance()" placeholder="输入患者主诉..." class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
          <button @click="sendUtterance()" class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium">发送</button>
        </div>
      </div>

      <!-- Right: ECS Component Inspector -->
      <div class="h-[540px] overflow-y-auto rounded-xl bg-slate-900 border border-slate-800 p-4 space-y-3 text-xs">
        <h3 class="font-semibold text-emerald-300 text-sm">ECS Inspector（实时 Component 透视）</h3>
        <div v-if="!latestTrace" class="text-slate-500">发送任意消息以查看 ECS 执行轨迹...</div>
        <template v-else>
          <div class="p-2.5 rounded bg-slate-950 border border-slate-800">
            <div class="text-slate-400 mb-1">Stage Timings (总耗时 {{ latestTrace.total_duration_ms }}ms · LLM调用 {{ latestTrace.llm_calls_made }} 次)</div>
            <div class="flex flex-wrap gap-1.5">
              <span v-for="st in latestTrace.stage_timings" :key="st.stage" class="px-2 py-0.5 rounded bg-slate-800">
                {{ st.stage }}: {{ st.duration_ms }}ms
              </span>
            </div>
          </div>
          <div class="p-2.5 rounded bg-slate-950 border border-slate-800">
            <div class="text-sky-300 font-semibold mb-1">ExtractedSymptoms & RedFlagAlert</div>
            <pre class="whitespace-pre-wrap text-slate-300">{{ JSON.stringify({ extracted_symptoms: latestTrace.extracted_symptoms, red_flag_alert: latestTrace.red_flag_alert }, null, 2) }}</pre>
          </div>
          <div class="p-2.5 rounded bg-slate-950 border border-slate-800">
            <div class="text-amber-300 font-semibold mb-1">TriageSlots & RagEvidence</div>
            <pre class="whitespace-pre-wrap text-slate-300">{{ JSON.stringify({ triage_slots: latestTrace.triage_slots, rag_hits: latestTrace.rag_evidence?.hits?.length ?? 0, seasonal: latestTrace.rag_evidence?.seasonal_context_applied }, null, 2) }}</pre>
          </div>
        </template>
      </div>
    </div>
  </section>
</template>
