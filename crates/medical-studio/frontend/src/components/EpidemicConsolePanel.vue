<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { invokeStudio } from '../api/tauri';

type SeasonKey = 'spring' | 'summer' | 'autumn' | 'winter' | 'all_year';

interface EpidemicContextItem {
  context_id: string;
  region_code: string;
  region_name: string;
  season: SeasonKey;
  disease_name: string;
  seasonal_tag: string;
  intensity: number;
  boost_symptoms: string[];
  priority_question_hint: string;
  active: boolean;
  valid_from: string;
  valid_to: string;
}

const seasonOptions: { value: SeasonKey; label: string }[] = [
  { value: 'spring', label: '春季 (spring)' },
  { value: 'summer', label: '夏季 (summer)' },
  { value: 'autumn', label: '秋季 (autumn)' },
  { value: 'winter', label: '冬季 (winter)' },
  { value: 'all_year', label: '全年 (all_year)' },
];

const seasonLabelMap: Record<SeasonKey, string> = {
  spring: '春季',
  summer: '夏季',
  autumn: '秋季',
  winter: '冬季',
  all_year: '全年',
};

const regionPresets = [
  { code: 'CN-000000', name: '全国通用' },
  { code: 'CN-110000', name: '北京市' },
  { code: 'CN-310000', name: '上海市' },
  { code: 'CN-440000', name: '广东省' },
  { code: 'CN-330000', name: '浙江省' },
  { code: 'CN-510000', name: '四川省' },
];

const epidemicTemplates: Array<{
  label: string;
  disease_name: string;
  seasonal_tag: string;
  season: SeasonKey;
  intensity: number;
  boost_symptoms: string[];
  priority_question_hint: string;
}> = [
  {
    label: '甲型流感',
    disease_name: '甲型流行性感冒',
    seasonal_tag: 'influenza',
    season: 'winter',
    intensity: 0.85,
    boost_symptoms: ['高热', '肌肉酸痛', '咽痛', '干咳', '乏力'],
    priority_question_hint: '近期身边有家人、同事或同学发烧吗？',
  },
  {
    label: '支原体肺炎',
    disease_name: '肺炎支原体感染',
    seasonal_tag: 'mycoplasma',
    season: 'autumn',
    intensity: 0.8,
    boost_symptoms: ['持续发热', '刺激性干咳', '夜间咳嗽加重', '胸闷'],
    priority_question_hint: '咳嗽是否以夜间阵发性干咳为主，持续超过5天了吗？',
  },
  {
    label: '诺如病毒胃肠炎',
    disease_name: '诺如病毒急性胃肠炎',
    seasonal_tag: 'norovirus',
    season: 'winter',
    intensity: 0.75,
    boost_symptoms: ['呕吐', '水样便', '腹痛', '恶心', '低热'],
    priority_question_hint: '近期有无外出就餐、生食海鲜，或同住人出现呕吐腹泻？',
  },
  {
    label: '登革热',
    disease_name: '登革热',
    seasonal_tag: 'dengue',
    season: 'summer',
    intensity: 0.7,
    boost_symptoms: ['突发高热', '剧烈头痛', '眼眶痛', '肌肉关节痛', '皮疹'],
    priority_question_hint: '近两周有无蚊虫叮咬史，或前往南方/东南亚地区旅居史？',
  },
  {
    label: '手足口病',
    disease_name: '手足口病',
    seasonal_tag: 'hfmd',
    season: 'summer',
    intensity: 0.75,
    boost_symptoms: ['发热', '口腔疱疹', '手足皮疹', '食欲减退'],
    priority_question_hint: '手心、脚心或口腔内是否出现红点或小水疱？',
  },
  {
    label: '春季花粉过敏',
    disease_name: '季节性过敏性鼻炎',
    seasonal_tag: 'pollen_allergy',
    season: 'spring',
    intensity: 0.65,
    boost_symptoms: ['阵发性打喷嚏', '清水样鼻涕', '鼻痒', '眼痒'],
    priority_question_hint: '症状是否在户外或接触花草后明显加重，既往有过敏史吗？',
  },
];

function formatDate(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

function defaultDateRange(): { from: string; to: string } {
  const start = new Date();
  const end = new Date(start.getTime() + 90 * 24 * 60 * 60 * 1000);
  return { from: formatDate(start), to: formatDate(end) };
}

const contexts = ref<EpidemicContextItem[]>([]);
const serverUrl = ref('http://127.0.0.1:8080');
const adminToken = ref('e2e-admin-secret');
const showAdminToken = ref(false);
const showServerSettings = ref(false);

const notice = ref('');
const noticeType = ref<'info' | 'success' | 'error'>('info');
const isBusy = ref(false);

// Search & filter state
const searchQuery = ref('');
const filterSeason = ref<'all' | SeasonKey>('all');
const onlyActive = ref(false);

// Create custom epidemic form state
const showCreateForm = ref(false);
const newSymptomInput = ref('');
const cardSymptomInputs = reactive<Record<string, string>>({});
const confirmingDeleteId = ref<string | null>(null);

const initialRange = defaultDateRange();
const newCtx = reactive<EpidemicContextItem>({
  context_id: '',
  region_code: 'CN-110000',
  region_name: '北京市',
  season: 'winter',
  disease_name: '',
  seasonal_tag: '',
  intensity: 0.75,
  boost_symptoms: [],
  priority_question_hint: '',
  active: true,
  valid_from: initialRange.from,
  valid_to: initialRange.to,
});

function setNotice(msg: string, type: 'info' | 'success' | 'error' = 'info') {
  notice.value = msg;
  noticeType.value = type;
}

function resetNewCtx() {
  const range = defaultDateRange();
  newCtx.context_id = '';
  newCtx.region_code = 'CN-110000';
  newCtx.region_name = '北京市';
  newCtx.season = 'winter';
  newCtx.disease_name = '';
  newCtx.seasonal_tag = '';
  newCtx.intensity = 0.75;
  newCtx.boost_symptoms = [];
  newCtx.priority_question_hint = '';
  newCtx.active = true;
  newCtx.valid_from = range.from;
  newCtx.valid_to = range.to;
  newSymptomInput.value = '';
}

function applyTemplate(tpl: (typeof epidemicTemplates)[number]) {
  newCtx.disease_name = tpl.disease_name;
  newCtx.seasonal_tag = tpl.seasonal_tag;
  newCtx.season = tpl.season;
  newCtx.intensity = tpl.intensity;
  newCtx.boost_symptoms = [...tpl.boost_symptoms];
  newCtx.priority_question_hint = tpl.priority_question_hint;
  const regionSlug = newCtx.region_code.toLowerCase().replace(/[^a-z0-9]+/g, '_');
  newCtx.context_id = `epi_${regionSlug}_${tpl.season}_${tpl.seasonal_tag}`;
  showCreateForm.value = true;
}

function selectRegionPreset(code: string, name: string) {
  newCtx.region_code = code;
  newCtx.region_name = name;
}

function parseTerms(raw: string): string[] {
  return raw
    .split(/[,，、;；\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function addNewFormSymptoms() {
  const terms = parseTerms(newSymptomInput.value);
  if (terms.length === 0) return;
  for (const t of terms) {
    if (!newCtx.boost_symptoms.includes(t)) {
      newCtx.boost_symptoms.push(t);
    }
  }
  newSymptomInput.value = '';
}

function removeNewFormSymptom(sym: string) {
  newCtx.boost_symptoms = newCtx.boost_symptoms.filter((s) => s !== sym);
}

function addCardSymptoms(ctx: EpidemicContextItem) {
  const raw = cardSymptomInputs[ctx.context_id] || '';
  const terms = parseTerms(raw);
  if (terms.length === 0) return;
  if (!Array.isArray(ctx.boost_symptoms)) {
    ctx.boost_symptoms = [];
  }
  for (const t of terms) {
    if (!ctx.boost_symptoms.includes(t)) {
      ctx.boost_symptoms.push(t);
    }
  }
  cardSymptomInputs[ctx.context_id] = '';
}

function removeCardSymptom(ctx: EpidemicContextItem, sym: string) {
  ctx.boost_symptoms = (ctx.boost_symptoms || []).filter((s) => s !== sym);
}

function intensityBadgeClass(val: number): string {
  if (val >= 0.75) return 'bg-rose-950/80 text-rose-300 border-rose-700/70';
  if (val >= 0.45) return 'bg-amber-950/80 text-amber-300 border-amber-700/70';
  return 'bg-emerald-950/80 text-emerald-300 border-emerald-700/70';
}

function intensityLevelLabel(val: number): string {
  if (val >= 0.75) return '高发预警';
  if (val >= 0.45) return '中度流行';
  return '低位监测';
}

const filteredContexts = computed(() => {
  const q = searchQuery.value.trim().toLowerCase();
  return contexts.value.filter((ctx) => {
    if (onlyActive.value && !ctx.active) return false;
    if (filterSeason.value !== 'all' && ctx.season !== filterSeason.value) return false;
    if (!q) return true;
    const haystack = [
      ctx.disease_name,
      ctx.context_id,
      ctx.region_name,
      ctx.region_code,
      ctx.seasonal_tag,
      ctx.priority_question_hint,
      ...(ctx.boost_symptoms || []),
    ]
      .join(' ')
      .toLowerCase();
    return haystack.includes(q);
  });
});

const activeCount = computed(() => contexts.value.filter((c) => c.active).length);

async function loadContexts() {
  try {
    contexts.value = await invokeStudio<EpidemicContextItem[]>('list_epidemic_contexts');
  } catch (err) {
    setNotice(`加载流行病配置失败：${String(err)}`, 'error');
  }
}

async function handleCreateContext(andReload = false) {
  // Flush pending symptom text if user didn't hit Enter
  if (newSymptomInput.value.trim()) {
    addNewFormSymptoms();
  }

  const diseaseName = newCtx.disease_name.trim();
  const regionCode = newCtx.region_code.trim();
  const regionName = newCtx.region_name.trim();
  if (!diseaseName) {
    setNotice('请填写疾病名称', 'error');
    return;
  }
  if (!regionCode || !regionName) {
    setNotice('请填写地区代码与地区名称', 'error');
    return;
  }
  if (newCtx.valid_from > newCtx.valid_to) {
    setNotice('生效开始日期不能晚于生效截止日期', 'error');
    return;
  }

  const seasonalTag =
    newCtx.seasonal_tag.trim() ||
    diseaseName
      .toLowerCase()
      .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, '_')
      .replace(/^_|_$/g, '') ||
    `tag_${Date.now()}`;

  const contextId =
    newCtx.context_id.trim() ||
    `epi_${regionCode.toLowerCase().replace(/[^a-z0-9]+/g, '_')}_${newCtx.season}_${Date.now().toString(36)}`;

  const payload: EpidemicContextItem = {
    context_id: contextId,
    region_code: regionCode,
    region_name: regionName,
    season: newCtx.season,
    disease_name: diseaseName,
    seasonal_tag: seasonalTag,
    intensity: Number(newCtx.intensity),
    boost_symptoms: [...newCtx.boost_symptoms],
    priority_question_hint: newCtx.priority_question_hint.trim(),
    active: newCtx.active,
    valid_from: newCtx.valid_from,
    valid_to: newCtx.valid_to,
  };

  isBusy.value = true;
  try {
    await invokeStudio<EpidemicContextItem>('upsert_epidemic_context', { ctx: payload });
    await loadContexts();
    resetNewCtx();
    showCreateForm.value = false;
    if (andReload) {
      await triggerHotReloadInternal(`已新增「${payload.disease_name}」并同步热更新`);
    } else {
      setNotice(`已新增自定义流行病配置：${payload.disease_name}（${payload.context_id}）`, 'success');
    }
  } catch (err) {
    setNotice(`新增流行病配置失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function saveContext(ctx: EpidemicContextItem, andReload = false) {
  if (cardSymptomInputs[ctx.context_id]?.trim()) {
    addCardSymptoms(ctx);
  }
  if (ctx.valid_from > ctx.valid_to) {
    setNotice(`「${ctx.disease_name}」生效开始日期不能晚于截止日期`, 'error');
    return;
  }
  isBusy.value = true;
  try {
    await invokeStudio('upsert_epidemic_context', {
      ctx: {
        ...ctx,
        intensity: Number(ctx.intensity),
      },
    });
    if (andReload) {
      await triggerHotReloadInternal(`已保存「${ctx.disease_name}」并推送热更新`);
    } else {
      setNotice(`已保存 ${ctx.disease_name}（强度 ${Number(ctx.intensity).toFixed(2)}）`, 'success');
    }
  } catch (err) {
    setNotice(`保存失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function deleteContext(ctx: EpidemicContextItem) {
  isBusy.value = true;
  try {
    await invokeStudio<boolean>('delete_epidemic_context', { contextId: ctx.context_id });
    confirmingDeleteId.value = null;
    await loadContexts();
    setNotice(`已删除流行病配置：${ctx.disease_name}（${ctx.context_id}）`, 'info');
  } catch (err) {
    setNotice(`删除失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function triggerHotReloadInternal(prefix?: string) {
  const res = await invokeStudio<any>('notify_server_hot_reload', {
    serverUrl: serverUrl.value,
    adminToken: adminToken.value,
  });
  const ok = res.remote_notified || !serverUrl.value.trim();
  setNotice(prefix ? `${prefix}：${res.message}` : res.message, ok ? 'success' : 'info');
}

async function triggerHotReload() {
  isBusy.value = true;
  try {
    await triggerHotReloadInternal();
  } catch (err) {
    setNotice(`热更新推送失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

onMounted(loadContexts);
</script>

<template>
  <section class="space-y-5">
    <!-- Header -->
    <header class="flex flex-wrap items-start justify-between gap-4">
      <div>
        <div class="flex items-center gap-2.5">
          <h2 class="text-xl font-semibold text-white">季节与流行病上下文控制台</h2>
          <span class="px-2 py-0.5 rounded-full text-xs bg-slate-800 text-slate-300 border border-slate-700">
            共 {{ contexts.length }} 项 · 生效中 {{ activeCount }} 项
          </span>
        </div>
        <p class="text-sm text-slate-400 mt-1">
          支持自定义新增任意地区/季节高发疾病（甲流仅为默认示例），调节流行强度（0.0 ~ 1.0）、关联症状与优先追问，秒级推送至 medical-server 热更新
        </p>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <button
          @click="showCreateForm = !showCreateForm"
          class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium text-white transition-colors"
        >
          {{ showCreateForm ? '收起新增表单' : '+ 新增自定义流行病' }}
        </button>
        <button
          @click="showServerSettings = !showServerSettings"
          class="px-3 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs text-slate-300 border border-slate-700 transition-colors"
        >
          {{ showServerSettings ? '隐藏推送设置' : '推送地址设置' }}
        </button>
        <button
          :disabled="isBusy"
          @click="triggerHotReload"
          class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-sm font-medium text-white transition-colors"
        >
          推送热更新
        </button>
      </div>
    </header>

    <!-- Collapsible Remote Server Settings -->
    <div
      v-if="showServerSettings"
      class="p-4 rounded-xl bg-slate-900 border border-slate-800 grid grid-cols-1 md:grid-cols-2 gap-3 text-xs"
    >
      <div>
        <label class="block text-slate-400 mb-1">medical-server 地址</label>
        <input
          v-model="serverUrl"
          placeholder="http://127.0.0.1:8080"
          class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 font-mono text-slate-200"
        />
      </div>
      <div>
        <label class="block text-slate-400 mb-1">Admin Bearer Token</label>
        <div class="flex gap-2">
          <input
            :type="showAdminToken ? 'text' : 'password'"
            v-model="adminToken"
            placeholder="e2e-admin-secret"
            autocomplete="off"
            class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 font-mono text-slate-200"
          />
          <button
            type="button"
            @click="showAdminToken = !showAdminToken"
            class="px-3 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-300"
          >
            {{ showAdminToken ? '隐藏' : '显示' }}
          </button>
        </div>
      </div>
    </div>

    <!-- Status Notice -->
    <div
      v-if="notice"
      :class="{
        'bg-emerald-950/60 border-emerald-700 text-emerald-300': noticeType === 'success',
        'bg-rose-950/60 border-rose-700 text-rose-300': noticeType === 'error',
        'bg-slate-900 border-slate-700 text-sky-300': noticeType === 'info',
      }"
      class="p-3 rounded-lg border text-sm flex items-center justify-between gap-2"
    >
      <span>{{ notice }}</span>
      <button @click="notice = ''" class="text-xs opacity-70 hover:opacity-100">✕</button>
    </div>

    <!-- Create Custom Epidemic Form -->
    <div
      v-if="showCreateForm"
      class="p-5 rounded-xl bg-slate-900 border border-sky-700/70 space-y-4 text-xs shadow-lg shadow-sky-950/30"
    >
      <div class="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 pb-3">
        <div>
          <h3 class="font-semibold text-white text-sm">新增自定义季节 / 流行病上下文</h3>
          <p class="text-slate-400 mt-0.5">可手动填写任意病种与地区，也可点击右侧模板快速填充后再按需修改</p>
        </div>
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-slate-400">快速模板：</span>
          <button
            v-for="tpl in epidemicTemplates"
            :key="tpl.seasonal_tag"
            type="button"
            @click="applyTemplate(tpl)"
            class="px-2.5 py-1 rounded-md bg-slate-800 hover:bg-sky-900/70 text-slate-200 border border-slate-700 hover:border-sky-600 transition-colors"
          >
            {{ tpl.label }}
          </button>
        </div>
      </div>

      <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
        <div>
          <label class="block text-slate-400 mb-1">疾病名称 <span class="text-rose-400">*</span></label>
          <input
            v-model="newCtx.disease_name"
            placeholder="例如：肺炎支原体感染 / 登革热 / 诺如病毒"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">知识库关联标签 (seasonal_tag)</label>
          <input
            v-model="newCtx.seasonal_tag"
            placeholder="例如：mycoplasma / norovirus（留空自动生成）"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 font-mono text-slate-100"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">上下文 ID (context_id)</label>
          <input
            v-model="newCtx.context_id"
            placeholder="留空自动生成唯一 ID"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 font-mono text-slate-100"
          />
        </div>

        <div>
          <label class="block text-slate-400 mb-1">适用地区名称 <span class="text-rose-400">*</span></label>
          <input
            v-model="newCtx.region_name"
            placeholder="例如：北京市 / 广东省 / 全国通用"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
          <div class="flex flex-wrap gap-1 mt-1.5">
            <button
              v-for="reg in regionPresets"
              :key="reg.code"
              type="button"
              @click="selectRegionPreset(reg.code, reg.name)"
              class="px-1.5 py-0.5 rounded bg-slate-800/80 hover:bg-slate-700 text-[11px] text-slate-300"
            >
              {{ reg.name }}
            </button>
          </div>
        </div>
        <div>
          <label class="block text-slate-400 mb-1">行政区划代码 (region_code) <span class="text-rose-400">*</span></label>
          <input
            v-model="newCtx.region_code"
            placeholder="例如：CN-110000"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 font-mono text-slate-100"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">高发季节</label>
          <select
            v-model="newCtx.season"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          >
            <option v-for="opt in seasonOptions" :key="opt.value" :value="opt.value">
              {{ opt.label }}
            </option>
          </select>
        </div>

        <div>
          <div class="flex justify-between mb-1">
            <label class="text-slate-400">流行强度权重</label>
            <span class="font-mono text-amber-300">{{ Number(newCtx.intensity).toFixed(2) }} · {{ intensityLevelLabel(newCtx.intensity) }}</span>
          </div>
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            v-model.number="newCtx.intensity"
            class="w-full accent-sky-500 mt-1.5"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">生效开始日期</label>
          <input
            type="date"
            v-model="newCtx.valid_from"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">生效截止日期</label>
          <input
            type="date"
            v-model="newCtx.valid_to"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
        </div>

        <div class="md:col-span-3">
          <label class="block text-slate-400 mb-1">关联高发症状（用于 RAG 召回加权与分诊匹配，回车或逗号批量添加）</label>
          <div class="flex gap-2">
            <input
              v-model="newSymptomInput"
              @keyup.enter.prevent="addNewFormSymptoms"
              placeholder="输入症状如：高热、干咳、皮疹（支持逗号/顿号分隔）"
              class="flex-1 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
            />
            <button
              type="button"
              @click="addNewFormSymptoms"
              class="px-3 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-sky-300 border border-slate-700"
            >
              + 添加症状
            </button>
          </div>
          <div v-if="newCtx.boost_symptoms.length > 0" class="flex flex-wrap gap-1.5 mt-2">
            <span
              v-for="sym in newCtx.boost_symptoms"
              :key="sym"
              class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full bg-indigo-950/80 text-indigo-200 border border-indigo-700/60"
            >
              {{ sym }}
              <button type="button" @click="removeNewFormSymptom(sym)" class="text-indigo-300 hover:text-rose-300">✕</button>
            </span>
          </div>
        </div>

        <div class="md:col-span-3">
          <label class="block text-slate-400 mb-1">优先追问引导语（ECS 槽位追问将优先结合此提示）</label>
          <input
            v-model="newCtx.priority_question_hint"
            placeholder="例如：近期是否有密切接触史、外出就餐史或夜间咳嗽加重？"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-slate-100"
          />
        </div>
      </div>

      <div class="flex flex-wrap items-center justify-between gap-3 pt-2 border-t border-slate-800">
        <label class="flex items-center gap-2 text-slate-200 cursor-pointer select-none">
          <input type="checkbox" v-model="newCtx.active" class="accent-sky-500" />
          创建后立即启用
        </label>
        <div class="flex items-center gap-2">
          <button
            type="button"
            @click="resetNewCtx"
            class="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300"
          >
            重置
          </button>
          <button
            type="button"
            @click="showCreateForm = false"
            class="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300"
          >
            取消
          </button>
          <button
            type="button"
            :disabled="isBusy"
            @click="handleCreateContext(false)"
            class="px-4 py-1.5 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 font-medium text-white"
          >
            保存配置
          </button>
          <button
            type="button"
            :disabled="isBusy"
            @click="handleCreateContext(true)"
            class="px-4 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 font-medium text-white"
          >
            保存并推送热更新
          </button>
        </div>
      </div>
    </div>

    <!-- Filter Toolbar -->
    <div class="flex flex-wrap items-center gap-3 p-3 rounded-xl bg-slate-900/80 border border-slate-800 text-xs">
      <input
        v-model="searchQuery"
        placeholder="搜索疾病名称、地区、标签或高发症状..."
        class="flex-1 min-w-[200px] px-3 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-slate-200"
      />
      <select
        v-model="filterSeason"
        class="px-3 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-slate-200"
      >
        <option value="all">全部季节</option>
        <option v-for="opt in seasonOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
      </select>
      <label class="flex items-center gap-1.5 text-slate-300 cursor-pointer select-none px-2">
        <input type="checkbox" v-model="onlyActive" class="accent-sky-500" />
        仅看已启用
      </label>
    </div>

    <!-- Empty State -->
    <div
      v-if="filteredContexts.length === 0"
      class="p-10 rounded-xl bg-slate-900/60 border border-dashed border-slate-700 text-center space-y-3"
    >
      <p class="text-sm text-slate-300">
        {{ contexts.length === 0 ? '暂无任何季节/流行病配置' : '没有匹配当前筛选条件的流行病配置' }}
      </p>
      <button
        @click="showCreateForm = true"
        class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-xs font-medium text-white"
      >
        + 新增自定义流行病配置
      </button>
    </div>

    <!-- Epidemic Context Cards -->
    <div v-else class="grid grid-cols-1 xl:grid-cols-2 gap-4">
      <div
        v-for="ctx in filteredContexts"
        :key="ctx.context_id"
        :class="ctx.active ? 'border-slate-700/90' : 'border-slate-800 opacity-75'"
        class="p-4 rounded-xl bg-slate-900 border space-y-3.5 text-xs transition-all"
      >
        <!-- Card Top Header -->
        <div class="flex flex-wrap items-start justify-between gap-2">
          <div class="space-y-1 flex-1 min-w-[200px]">
            <div class="flex items-center gap-2">
              <input
                v-model="ctx.disease_name"
                class="font-semibold text-white text-sm bg-transparent border-b border-transparent hover:border-slate-700 focus:border-sky-500 focus:outline-none px-0.5 py-0.5 w-full max-w-[240px]"
              />
              <span
                :class="intensityBadgeClass(ctx.intensity)"
                class="px-2 py-0.5 rounded-full border text-[11px] shrink-0"
              >
                {{ intensityLevelLabel(ctx.intensity) }}
              </span>
            </div>
            <div class="flex flex-wrap items-center gap-1.5 text-[11px] text-slate-400">
              <span class="font-mono bg-slate-950 px-1.5 py-0.5 rounded border border-slate-800">{{ ctx.context_id }}</span>
              <span>·</span>
              <span>{{ ctx.region_name }} ({{ ctx.region_code }})</span>
              <span>·</span>
              <span>{{ seasonLabelMap[ctx.season] || ctx.season }}</span>
              <span>·</span>
              <span class="text-amber-300">#{{ ctx.seasonal_tag }}</span>
            </div>
          </div>
          <label class="flex items-center gap-1.5 text-xs text-slate-200 cursor-pointer select-none shrink-0 pt-1">
            <input type="checkbox" v-model="ctx.active" class="accent-emerald-500" />
            {{ ctx.active ? '已启用' : '已停用' }}
          </label>
        </div>

        <!-- Region / Season / Tag Quick Edit -->
        <div class="grid grid-cols-3 gap-2">
          <div>
            <label class="block text-slate-400 mb-1">地区名称</label>
            <input
              v-model="ctx.region_name"
              class="w-full px-2 py-1 rounded bg-slate-950 border border-slate-700 text-slate-200"
            />
          </div>
          <div>
            <label class="block text-slate-400 mb-1">地区代码</label>
            <input
              v-model="ctx.region_code"
              class="w-full px-2 py-1 rounded bg-slate-950 border border-slate-700 font-mono text-slate-200"
            />
          </div>
          <div>
            <label class="block text-slate-400 mb-1">高发季节</label>
            <select
              v-model="ctx.season"
              class="w-full px-2 py-1 rounded bg-slate-950 border border-slate-700 text-slate-200"
            >
              <option v-for="opt in seasonOptions" :key="opt.value" :value="opt.value">
                {{ seasonLabelMap[opt.value] }}
              </option>
            </select>
          </div>
        </div>

        <!-- Intensity Slider -->
        <div>
          <div class="flex justify-between mb-1">
            <span class="text-slate-400">流行强度权重</span>
            <span class="font-mono text-amber-300">{{ Number(ctx.intensity).toFixed(2) }}</span>
          </div>
          <input
            type="range"
            min="0"
            max="1"
            step="0.05"
            v-model.number="ctx.intensity"
            class="w-full accent-sky-500"
          />
        </div>

        <!-- Priority Question Hint -->
        <div>
          <label class="block text-slate-400 mb-1">优先追问引导语</label>
          <input
            v-model="ctx.priority_question_hint"
            placeholder="例如：近期身边有家人或同事发烧吗？"
            class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 text-slate-200"
          />
        </div>

        <!-- Date Range & Tag -->
        <div class="grid grid-cols-3 gap-2">
          <div>
            <label class="block text-slate-400 mb-1">生效开始</label>
            <input
              type="date"
              v-model="ctx.valid_from"
              class="w-full px-2 py-1 rounded bg-slate-950 border border-slate-700 text-slate-200"
            />
          </div>
          <div>
            <label class="block text-slate-400 mb-1">生效截止</label>
            <input
              type="date"
              v-model="ctx.valid_to"
              class="w-full px-2 py-1 rounded bg-slate-950 border border-slate-700 text-slate-200"
            />
          </div>
          <div>
            <label class="block text-slate-400 mb-1">关联标签</label>
            <input
              v-model="ctx.seasonal_tag"
              class="w-full px-2 py-1 rounded bg-slate-950 border border-slate-700 font-mono text-slate-200"
            />
          </div>
        </div>

        <!-- Editable Boost Symptoms Tags -->
        <div class="space-y-1.5">
          <label class="block text-slate-400">关联高发症状</label>
          <div class="flex flex-wrap gap-1.5">
            <span
              v-for="sym in ctx.boost_symptoms || []"
              :key="sym"
              class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-indigo-950/70 text-indigo-200 border border-indigo-800/60"
            >
              {{ sym }}
              <button type="button" @click="removeCardSymptom(ctx, sym)" class="hover:text-rose-300">✕</button>
            </span>
            <span v-if="!ctx.boost_symptoms || ctx.boost_symptoms.length === 0" class="text-slate-500">
              暂无关联症状
            </span>
          </div>
          <div class="flex gap-1.5 pt-1">
            <input
              v-model="cardSymptomInputs[ctx.context_id]"
              @keyup.enter.prevent="addCardSymptoms(ctx)"
              placeholder="新增症状（逗号分隔，回车添加）"
              class="flex-1 px-2.5 py-1 rounded bg-slate-950 border border-slate-700 text-slate-200"
            />
            <button
              type="button"
              @click="addCardSymptoms(ctx)"
              class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-sky-300 border border-slate-700"
            >
              添加
            </button>
          </div>
        </div>

        <!-- Card Actions -->
        <div class="flex flex-wrap items-center justify-between gap-2 pt-2 border-t border-slate-800">
          <div class="flex items-center gap-2">
            <button
              :disabled="isBusy"
              @click="saveContext(ctx, false)"
              class="px-3 py-1.5 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 font-medium text-white"
            >
              保存配置
            </button>
            <button
              :disabled="isBusy"
              @click="saveContext(ctx, true)"
              class="px-3 py-1.5 rounded-lg bg-emerald-600/90 hover:bg-emerald-500 disabled:opacity-50 font-medium text-white"
            >
              保存并热更新
            </button>
          </div>

          <div v-if="confirmingDeleteId === ctx.context_id" class="flex items-center gap-1.5">
            <span class="text-rose-300">确认删除？</span>
            <button
              :disabled="isBusy"
              @click="deleteContext(ctx)"
              class="px-2.5 py-1 rounded bg-rose-600 hover:bg-rose-500 text-white font-medium"
            >
              确定
            </button>
            <button
              @click="confirmingDeleteId = null"
              class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-slate-300"
            >
              取消
            </button>
          </div>
          <button
            v-else
            @click="confirmingDeleteId = ctx.context_id"
            class="px-2.5 py-1.5 rounded-lg bg-slate-800 hover:bg-rose-900/70 text-rose-300 border border-slate-700"
          >
            删除
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
