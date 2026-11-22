<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';

const toast = useToast();

const dictionaries = ref<{ symptom_synonyms: any[]; red_flag_rules: any[] }>({
  symptom_synonyms: [],
  red_flag_rules: [],
});

const sandboxInput = ref('老人突然心口窝撕裂样疼出冷汗');
const sandboxResult = ref<any>(null);
const isBusy = ref(false);

const synSearch = ref('');
const ruleSearch = ref('');

const newStdTerm = ref('');
const newColloquials = ref('');

const newRuleId = ref('');
const newRuleLevel = ref<'Emergency120' | 'Urgent'>('Emergency120');
const newTriggerAll = ref('');
const newTriggerAny = ref('');
const newAlertMsg = ref('');

const confirmingSyn = ref<string | null>(null);
const confirmingRule = ref<string | null>(null);

const filteredSynonyms = computed(() => {
  const q = synSearch.value.trim().toLowerCase();
  if (!q) return dictionaries.value.symptom_synonyms;
  return dictionaries.value.symptom_synonyms.filter(
    (s) =>
      s.standard_term.toLowerCase().includes(q) ||
      (s.colloquial_terms || []).some((t: string) => t.toLowerCase().includes(q)),
  );
});

const filteredRules = computed(() => {
  const q = ruleSearch.value.trim().toLowerCase();
  if (!q) return dictionaries.value.red_flag_rules;
  return dictionaries.value.red_flag_rules.filter((r) => {
    const hay = [
      r.rule_id,
      r.level,
      r.alert_message,
      ...(r.trigger_all_of || []),
      ...(r.trigger_any_of || []),
    ]
      .join(' ')
      .toLowerCase();
    return hay.includes(q);
  });
});

function splitTerms(raw: string): string[] {
  return raw
    .split(/[,，、;；]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

async function refresh() {
  try {
    dictionaries.value = await invokeStudio('get_medical_dictionaries');
  } catch (err) {
    toast.error(`加载词典失败：${String(err)}`);
  }
}

function editSynonym(syn: any) {
  newStdTerm.value = syn.standard_term;
  newColloquials.value = (syn.colloquial_terms || []).join(', ');
}

function editRedFlag(rule: any) {
  newRuleId.value = rule.rule_id;
  newRuleLevel.value = rule.level === 'Urgent' ? 'Urgent' : 'Emergency120';
  newTriggerAll.value = (rule.trigger_all_of || []).join(', ');
  newTriggerAny.value = (rule.trigger_any_of || []).join(', ');
  newAlertMsg.value = rule.alert_message || '';
}

async function addSynonym() {
  const std = newStdTerm.value.trim();
  const cols = splitTerms(newColloquials.value);
  if (!std || cols.length === 0) {
    toast.warning('请填写标准术语及至少一个口语同义词');
    return;
  }
  isBusy.value = true;
  try {
    await invokeStudio('save_symptom_synonym', {
      synonym: {
        standard_term: std,
        colloquial_terms: cols,
      },
    });
    newStdTerm.value = '';
    newColloquials.value = '';
    await refresh();
    toast.success(`已保存同义词映射：${std}`);
  } catch (err) {
    toast.error(`保存同义词失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

async function removeSynonym(term: string) {
  isBusy.value = true;
  try {
    await invokeStudio('delete_symptom_synonym', { standardTerm: term });
    confirmingSyn.value = null;
    await refresh();
    toast.info(`已删除同义词：${term}`);
  } catch (err) {
    toast.error(`删除同义词失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

async function addRedFlag() {
  const ruleId = newRuleId.value.trim() || `rf_custom_${Date.now().toString(36)}`;
  const allOf = splitTerms(newTriggerAll.value);
  const anyOf = splitTerms(newTriggerAny.value);
  if (allOf.length === 0 && anyOf.length === 0) {
    toast.warning('请至少填写一组必含关键词 (ALL) 或任一关键词 (ANY)');
    return;
  }
  isBusy.value = true;
  try {
    await invokeStudio('save_red_flag_rule', {
      rule: {
        rule_id: ruleId,
        level: newRuleLevel.value,
        trigger_all_of: allOf,
        trigger_any_of: anyOf,
        alert_message:
          newAlertMsg.value.trim() || '检测到急危重症指征，请立即拨打 120 急救电话！',
      },
    });
    newRuleId.value = '';
    newTriggerAll.value = '';
    newTriggerAny.value = '';
    newAlertMsg.value = '';
    await refresh();
    toast.success(`已保存红线规则：${ruleId}`);
  } catch (err) {
    toast.error(`保存红线规则失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

async function removeRedFlag(ruleId: string) {
  isBusy.value = true;
  try {
    await invokeStudio('delete_red_flag_rule', { ruleId });
    confirmingRule.value = null;
    await refresh();
    toast.info(`已删除红线规则：${ruleId}`);
  } catch (err) {
    toast.error(`删除红线规则失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

async function runSandbox() {
  if (!sandboxInput.value.trim()) {
    toast.warning('请输入测试主诉文本');
    return;
  }
  isBusy.value = true;
  try {
    sandboxResult.value = await invokeStudio('test_dictionary_sandbox', {
      utterance: sandboxInput.value,
    });
  } catch (err) {
    toast.error(`沙盒测试失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

onMounted(refresh);
</script>

<template>
  <section class="space-y-5">
    <header class="flex flex-wrap items-center justify-between gap-3">
      <div>
        <div class="flex items-center gap-2.5">
          <h2 class="text-xl font-semibold text-white">关键词与急危重症词典管理器</h2>
          <span class="px-2 py-0.5 rounded-full text-xs bg-slate-800 text-slate-300 border border-slate-700">
            同义词 {{ dictionaries.symptom_synonyms.length }} · 红线 {{ dictionaries.red_flag_rules.length }}
          </span>
        </div>
        <p class="text-sm text-slate-400 mt-1">维护口语同义词表与 120 急诊红线组合规则，提供实时词法与急诊拦截测试沙盒</p>
      </div>
    </header>

    <!-- Live Sandbox -->
    <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold text-amber-300">实时词法与红线拦截测试沙盒</h3>
        <div class="flex gap-1.5 text-[11px]">
          <button
            @click="sandboxInput = '老人突然心口窝撕裂样疼出冷汗'; runSandbox()"
            class="px-2 py-0.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-300"
          >
            示例：急性胸痛
          </button>
          <button
            @click="sandboxInput = '拉水一天了肚子绞痛还想吐'; runSandbox()"
            class="px-2 py-0.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-300"
          >
            示例：口语腹泻
          </button>
        </div>
      </div>
      <div class="flex gap-2">
        <input
          v-model="sandboxInput"
          @keyup.enter="runSandbox"
          placeholder="输入患者口语主诉，回车立即测试..."
          class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm text-slate-100"
        />
        <button
          :disabled="isBusy"
          @click="runSandbox"
          class="px-4 py-2 rounded-lg bg-amber-600 hover:bg-amber-500 disabled:opacity-50 text-sm font-medium text-white"
        >
          {{ isBusy ? '测试中...' : '测试命中' }}
        </button>
      </div>
      <div v-if="sandboxResult" class="p-3 rounded-lg bg-slate-950 border border-slate-800 text-xs space-y-2">
        <div class="flex flex-wrap items-center gap-2">
          <span class="text-slate-400">标准术语命中：</span>
          <span
            v-for="term in sandboxResult.standardized_terms"
            :key="term"
            class="px-2 py-0.5 rounded-full bg-sky-900/70 text-sky-200 border border-sky-700/60"
          >
            {{ term }}
          </span>
          <span v-if="!sandboxResult.standardized_terms?.length" class="text-slate-500">无</span>
        </div>
        <div class="flex items-center gap-2">
          <span class="text-slate-400">急诊等级：</span>
          <span
            :class="
              sandboxResult.red_flag_level === 'Emergency120'
                ? 'bg-rose-950 text-rose-300 border-rose-700 font-bold'
                : sandboxResult.red_flag_level === 'Urgent'
                  ? 'bg-amber-950 text-amber-300 border-amber-700 font-semibold'
                  : 'bg-emerald-950 text-emerald-300 border-emerald-700'
            "
            class="px-2 py-0.5 rounded-full border"
          >
            {{ sandboxResult.red_flag_level }}
          </span>
          <span v-if="sandboxResult.matched_rule_ids?.length" class="text-slate-400 font-mono">
            命中规则：{{ sandboxResult.matched_rule_ids.join(', ') }}
          </span>
        </div>
        <div v-if="sandboxResult.alert_message" class="p-2 rounded bg-rose-950/60 border border-rose-800 text-rose-200">
          {{ sandboxResult.alert_message }}
        </div>
      </div>
    </div>

    <!-- Two Column Editors -->
    <div class="grid grid-cols-1 xl:grid-cols-2 gap-4">
      <!-- Synonyms -->
      <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 flex flex-col">
        <div class="flex items-center justify-between gap-2">
          <h3 class="font-semibold text-white text-sm">口语 → 标准术语同义词</h3>
          <input
            v-model="synSearch"
            placeholder="过滤术语..."
            class="px-2.5 py-1 rounded-lg bg-slate-950 border border-slate-700 text-xs w-40 text-slate-200"
          />
        </div>
        <div class="flex gap-2">
          <input
            v-model="newStdTerm"
            placeholder="标准术语（如：胸痛）"
            class="w-1/3 px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs text-slate-100"
          />
          <input
            v-model="newColloquials"
            @keyup.enter="addSynonym"
            placeholder="口语词（逗号/顿号分隔）"
            class="flex-1 px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs text-slate-100"
          />
          <button
            :disabled="isBusy"
            @click="addSynonym"
            class="px-3 py-1.5 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-xs font-medium text-white shrink-0"
          >
            保存
          </button>
        </div>
        <div class="space-y-1.5 max-h-[380px] overflow-y-auto pr-1">
          <div
            v-for="syn in filteredSynonyms"
            :key="syn.standard_term"
            class="flex justify-between items-center gap-2 p-2.5 rounded-lg bg-slate-950 border border-slate-800/80 text-xs hover:border-slate-700 transition-colors"
          >
            <div class="min-w-0 flex-1">
              <strong class="text-sky-300">{{ syn.standard_term }}</strong>
              <span class="text-slate-500 mx-1.5">←</span>
              <span class="text-slate-300">{{ (syn.colloquial_terms || []).join(' / ') }}</span>
            </div>
            <div class="flex items-center gap-2 shrink-0">
              <button @click="editSynonym(syn)" class="text-slate-400 hover:text-sky-300">编辑</button>
              <template v-if="confirmingSyn === syn.standard_term">
                <button @click="removeSynonym(syn.standard_term)" class="text-rose-400 font-semibold">确认</button>
                <button @click="confirmingSyn = null" class="text-slate-400">取消</button>
              </template>
              <button v-else @click="confirmingSyn = syn.standard_term" class="text-rose-400 hover:underline">删除</button>
            </div>
          </div>
          <div v-if="filteredSynonyms.length === 0" class="text-center py-6 text-xs text-slate-500">
            没有匹配的同义词条目
          </div>
        </div>
      </div>

      <!-- Red Flag Rules -->
      <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 flex flex-col">
        <div class="flex items-center justify-between gap-2">
          <h3 class="font-semibold text-white text-sm">急危重症红线组合规则</h3>
          <input
            v-model="ruleSearch"
            placeholder="过滤规则..."
            class="px-2.5 py-1 rounded-lg bg-slate-950 border border-slate-700 text-xs w-40 text-slate-200"
          />
        </div>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-2 text-xs">
          <input
            v-model="newRuleId"
            placeholder="规则 ID（留空自动生成）"
            class="px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 font-mono text-slate-100"
          />
          <select
            v-model="newRuleLevel"
            class="px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          >
            <option value="Emergency120">Emergency120（立即拨打120）</option>
            <option value="Urgent">Urgent（急诊优先）</option>
          </select>
          <input
            v-model="newTriggerAll"
            placeholder="必含关键词 ALL（逗号分隔）"
            class="px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
          <input
            v-model="newTriggerAny"
            placeholder="任一伴随词 ANY（可选，逗号分隔）"
            class="px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
          <div class="sm:col-span-2 flex gap-2">
            <input
              v-model="newAlertMsg"
              @keyup.enter="addRedFlag"
              placeholder="急诊预警提示文案（默认：检测到急危重症指征，请立即拨打 120！）"
              class="flex-1 px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
            />
            <button
              :disabled="isBusy"
              @click="addRedFlag"
              class="px-3.5 py-1.5 rounded-lg bg-rose-600 hover:bg-rose-500 disabled:opacity-50 text-xs font-medium text-white shrink-0"
            >
              保存红线
            </button>
          </div>
        </div>
        <div class="space-y-1.5 max-h-[340px] overflow-y-auto pr-1">
          <div
            v-for="rule in filteredRules"
            :key="rule.rule_id"
            class="p-2.5 rounded-lg bg-slate-950 border border-slate-800/80 text-xs space-y-1 hover:border-slate-700 transition-colors"
          >
            <div class="flex justify-between items-center gap-2">
              <div class="flex items-center gap-1.5 min-w-0">
                <span
                  :class="rule.level === 'Emergency120' ? 'bg-rose-950 text-rose-300 border-rose-700' : 'bg-amber-950 text-amber-300 border-amber-700'"
                  class="px-1.5 py-0.5 rounded border text-[11px] font-semibold shrink-0"
                >
                  {{ rule.level }}
                </span>
                <span class="font-mono text-slate-200 truncate">{{ rule.rule_id }}</span>
              </div>
              <div class="flex items-center gap-2 shrink-0">
                <button @click="editRedFlag(rule)" class="text-slate-400 hover:text-sky-300">编辑</button>
                <template v-if="confirmingRule === rule.rule_id">
                  <button @click="removeRedFlag(rule.rule_id)" class="text-rose-400 font-semibold">确认</button>
                  <button @click="confirmingRule = null" class="text-slate-400">取消</button>
                </template>
                <button v-else @click="confirmingRule = rule.rule_id" class="text-rose-400 hover:underline">删除</button>
              </div>
            </div>
            <div class="text-slate-300">
              <span v-if="rule.trigger_all_of?.length">必含：<span class="text-amber-300">{{ rule.trigger_all_of.join(' + ') }}</span></span>
              <span v-if="rule.trigger_any_of?.length" class="ml-2">任一：<span class="text-sky-300">{{ rule.trigger_any_of.join(' | ') }}</span></span>
            </div>
            <div v-if="rule.alert_message" class="text-slate-400 truncate">{{ rule.alert_message }}</div>
          </div>
          <div v-if="filteredRules.length === 0" class="text-center py-6 text-xs text-slate-500">
            没有匹配的红线规则
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
