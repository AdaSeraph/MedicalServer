<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';

const dictionaries = ref<{ symptom_synonyms: any[]; red_flag_rules: any[] }>({
  symptom_synonyms: [],
  red_flag_rules: [],
});
const sandboxInput = ref('老人突然心口窝撕裂样疼出冷汗');
const sandboxResult = ref<any>(null);

const newStdTerm = ref('');
const newColloquials = ref('');
const newRuleId = ref('');
const newTriggerAll = ref('');
const newAlertMsg = ref('');

async function refresh() {
  dictionaries.value = await invokeStudio('get_medical_dictionaries');
}

async function addSynonym() {
  if (!newStdTerm.value.trim()) return;
  await invokeStudio('save_symptom_synonym', {
    synonym: {
      standard_term: newStdTerm.value.trim(),
      colloquial_terms: newColloquials.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean),
    },
  });
  newStdTerm.value = '';
  newColloquials.value = '';
  await refresh();
}

async function removeSynonym(term: string) {
  await invokeStudio('delete_symptom_synonym', { standardTerm: term });
  await refresh();
}

async function addRedFlag() {
  if (!newRuleId.value.trim()) return;
  await invokeStudio('save_red_flag_rule', {
    rule: {
      rule_id: newRuleId.value.trim(),
      level: 'Emergency120',
      trigger_all_of: newTriggerAll.value.split(/[,，]/).map((s) => s.trim()).filter(Boolean),
      trigger_any_of: [],
      alert_message: newAlertMsg.value.trim() || '检测到急危重症指征，请立即拨打 120！',
    },
  });
  newRuleId.value = '';
  newTriggerAll.value = '';
  newAlertMsg.value = '';
  await refresh();
}

async function removeRedFlag(ruleId: string) {
  await invokeStudio('delete_red_flag_rule', { ruleId });
  await refresh();
}

async function runSandbox() {
  sandboxResult.value = await invokeStudio('test_dictionary_sandbox', {
    utterance: sandboxInput.value,
  });
}

onMounted(refresh);
</script>

<template>
  <section class="space-y-6">
    <header>
      <h2 class="text-xl font-semibold text-white">关键词与急危重症词典管理器</h2>
      <p class="text-sm text-slate-400">维护口语同义词表与 120 急诊红线组合规则，并提供实时词法测试沙盒</p>
    </header>

    <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3">
      <h3 class="text-sm font-semibold text-amber-300">实时测试沙盒</h3>
      <div class="flex gap-2">
        <input v-model="sandboxInput" class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
        <button @click="runSandbox" class="px-4 py-2 rounded-lg bg-amber-600 hover:bg-amber-500 text-sm font-medium">测试命中</button>
      </div>
      <div v-if="sandboxResult" class="p-3 rounded-lg bg-slate-950 border border-slate-800 text-xs space-y-1">
        <div>标准术语命中：<span class="text-sky-300">{{ sandboxResult.standardized_terms.join('、') || '无' }}</span></div>
        <div>
          急诊等级：
          <span :class="sandboxResult.red_flag_level === 'Emergency120' ? 'text-rose-400 font-bold' : 'text-emerald-400'">
            {{ sandboxResult.red_flag_level }}
          </span>
        </div>
        <div v-if="sandboxResult.alert_message" class="text-rose-300">{{ sandboxResult.alert_message }}</div>
      </div>
    </div>

    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3">
        <h3 class="font-semibold text-white text-sm">口语 → 标准术语同义词</h3>
        <div class="flex gap-2">
          <input v-model="newStdTerm" placeholder="标准术语" class="w-1/3 px-2 py-1.5 rounded bg-slate-950 border border-slate-700 text-xs" />
          <input v-model="newColloquials" placeholder="口语词（逗号分隔）" class="flex-1 px-2 py-1.5 rounded bg-slate-950 border border-slate-700 text-xs" />
          <button @click="addSynonym" class="px-3 py-1.5 rounded bg-sky-600 text-xs">保存</button>
        </div>
        <div class="space-y-1.5 max-h-64 overflow-y-auto">
          <div v-for="syn in dictionaries.symptom_synonyms" :key="syn.standard_term" class="flex justify-between items-center p-2 rounded bg-slate-950 text-xs">
            <span><strong class="text-sky-300">{{ syn.standard_term }}</strong> ← {{ syn.colloquial_terms.join(' / ') }}</span>
            <button @click="removeSynonym(syn.standard_term)" class="text-rose-400 hover:underline">删除</button>
          </div>
        </div>
      </div>

      <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3">
        <h3 class="font-semibold text-white text-sm">急危重症 120 红线规则</h3>
        <div class="grid grid-cols-3 gap-2">
          <input v-model="newRuleId" placeholder="规则ID" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700 text-xs" />
          <input v-model="newTriggerAll" placeholder="必含关键词(逗号分隔)" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700 text-xs" />
          <button @click="addRedFlag" class="px-3 py-1.5 rounded bg-rose-600 text-xs">添加红线</button>
        </div>
        <div class="space-y-1.5 max-h-64 overflow-y-auto">
          <div v-for="rule in dictionaries.red_flag_rules" :key="rule.rule_id" class="flex justify-between items-center p-2 rounded bg-slate-950 text-xs">
            <span><strong class="text-rose-400">[{{ rule.level }}]</strong> {{ rule.rule_id }} · {{ rule.trigger_all_of.join('+') }}</span>
            <button @click="removeRedFlag(rule.rule_id)" class="text-rose-400 hover:underline">删除</button>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
