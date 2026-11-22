<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue';
import { invokeStudio } from '../api/tauri';

interface ProviderConfig {
  id: string;
  name: string;
  kind: string;
  base_url: string;
  api_key: string;
  model: string;
  tier: 'fast' | 'reasoning';
  timeout_ms: number;
  is_intranet: boolean;
  enabled: boolean;
  priority: number;
}

interface RouterConfig {
  fast_chain: string[];
  reasoning_chain: string[];
  default_timeout_ms: number;
  circuit_breaker_failure_threshold: number;
  circuit_breaker_cooldown_secs: number;
}

const providerKinds = [
  { value: 'openai_compatible', label: 'OpenAI 兼容接口' },
  { value: 'deepseek', label: 'DeepSeek' },
  { value: 'qwen', label: '通义千问 (Qwen)' },
  { value: 'gemini_openai', label: 'Gemini (OpenAI 兼容)' },
  { value: 'ollama', label: 'Ollama (本地)' },
  { value: 'vllm', label: 'vLLM (院内私有化)' },
  { value: 'custom_http', label: '自定义 HTTP' },
];

const presets = [
  {
    label: 'DeepSeek',
    id: 'deepseek_custom',
    name: 'DeepSeek Chat',
    kind: 'deepseek',
    base_url: 'https://api.deepseek.com/v1',
    model: 'deepseek-chat',
    tier: 'fast' as const,
    is_intranet: false,
  },
  {
    label: '通义千问 Qwen',
    id: 'qwen_dashscope',
    name: '通义千问 DashScope',
    kind: 'qwen',
    base_url: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    model: 'qwen-plus',
    tier: 'reasoning' as const,
    is_intranet: false,
  },
  {
    label: 'OpenAI',
    id: 'openai_custom',
    name: 'OpenAI GPT',
    kind: 'openai_compatible',
    base_url: 'https://api.openai.com/v1',
    model: 'gpt-4o-mini',
    tier: 'fast' as const,
    is_intranet: false,
  },
  {
    label: 'Gemini',
    id: 'gemini_custom',
    name: 'Google Gemini',
    kind: 'gemini_openai',
    base_url: 'https://generativelanguage.googleapis.com/v1beta/openai',
    model: 'gemini-2.5-flash',
    tier: 'fast' as const,
    is_intranet: false,
  },
  {
    label: '本地 Ollama',
    id: 'ollama_local',
    name: 'Local Ollama',
    kind: 'ollama',
    base_url: 'http://127.0.0.1:11434/v1',
    model: 'qwen2.5:14b',
    tier: 'fast' as const,
    is_intranet: true,
  },
];

const providers = ref<ProviderConfig[]>([]);
const routerConfig = ref<RouterConfig>({
  fast_chain: [],
  reasoning_chain: [],
  default_timeout_ms: 8000,
  circuit_breaker_failure_threshold: 3,
  circuit_breaker_cooldown_secs: 30,
});
const fastChainText = ref('');
const reasoningChainText = ref('');
const apiKeys = ref<any[]>([]);
const metrics = ref<any>(null);

const notice = ref('');
const noticeType = ref<'info' | 'success' | 'error'>('info');
const newlyIssuedKey = ref('');

const showApiKeyMap = reactive<Record<string, boolean>>({});
const probingMap = reactive<Record<string, boolean>>({});
const probeStatusMap = reactive<Record<string, { ok: boolean; text: string }>>({});

const showAddForm = ref(false);
const showNewApiKey = ref(false);
const newProvider = reactive<ProviderConfig>({
  id: '',
  name: '',
  kind: 'openai_compatible',
  base_url: '',
  api_key: '',
  model: '',
  tier: 'fast',
  timeout_ms: 8000,
  is_intranet: false,
  enabled: true,
  priority: 1,
});

const newTenantId = ref('his_clinic_01');
const newKeyName = ref('院内导诊机');
const newQpm = ref(120);
const newTpm = ref(100000);

function setNotice(message: string, type: 'info' | 'success' | 'error' = 'info') {
  notice.value = message;
  noticeType.value = type;
}

function applyPreset(preset: (typeof presets)[number]) {
  newProvider.id = preset.id;
  newProvider.name = preset.name;
  newProvider.kind = preset.kind;
  newProvider.base_url = preset.base_url;
  newProvider.model = preset.model;
  newProvider.tier = preset.tier;
  newProvider.is_intranet = preset.is_intranet;
  showAddForm.value = true;
}

function resetNewProvider() {
  newProvider.id = '';
  newProvider.name = '';
  newProvider.kind = 'openai_compatible';
  newProvider.base_url = '';
  newProvider.api_key = '';
  newProvider.model = '';
  newProvider.tier = 'fast';
  newProvider.timeout_ms = 8000;
  newProvider.is_intranet = false;
  newProvider.enabled = true;
  newProvider.priority = 1;
}

async function loadAll() {
  try {
    providers.value = await invokeStudio<ProviderConfig[]>('list_llm_providers');
    routerConfig.value = await invokeStudio<RouterConfig>('get_router_config');
    fastChainText.value = (routerConfig.value.fast_chain || []).join(', ');
    reasoningChainText.value = (routerConfig.value.reasoning_chain || []).join(', ');
    apiKeys.value = await invokeStudio<any[]>('list_api_keys');
    metrics.value = await invokeStudio<any>('get_gateway_metrics', { windowHours: 24 });
  } catch (err) {
    setNotice(`加载配置失败：${String(err)}`, 'error');
  }
}

async function saveProvider(cfg: ProviderConfig) {
  try {
    const saved = await invokeStudio<ProviderConfig>('upsert_llm_provider', { config: cfg });
    setNotice(`已保存 AI 模型配置：${saved.name} (${saved.model})`, 'success');
    await loadAll();
  } catch (err) {
    setNotice(`保存配置失败：${String(err)}`, 'error');
  }
}

async function handleAddProvider() {
  try {
    const payload: ProviderConfig = {
      ...newProvider,
      id: newProvider.id.trim() || `provider_${Date.now()}`,
      name: newProvider.name.trim() || newProvider.model.trim() || '自定义模型',
      base_url: newProvider.base_url.trim(),
      api_key: newProvider.api_key.trim(),
      model: newProvider.model.trim(),
    };
    const saved = await invokeStudio<ProviderConfig>('upsert_llm_provider', { config: payload });

    // Append to matching tier routing chain if not already present
    const chain = saved.tier === 'fast' ? routerConfig.value.fast_chain : routerConfig.value.reasoning_chain;
    if (!chain.includes(saved.id)) {
      chain.push(saved.id);
      await invokeStudio('save_router_config', { config: routerConfig.value });
    }

    setNotice(`已新增 AI 模型 Provider：${saved.name} (${saved.id})`, 'success');
    resetNewProvider();
    showAddForm.value = false;
    await loadAll();
  } catch (err) {
    setNotice(`新增 Provider 失败：${String(err)}`, 'error');
  }
}

async function removeProvider(id: string) {
  try {
    await invokeStudio('delete_llm_provider', { providerId: id });
    delete probeStatusMap[id];
    setNotice(`已删除 Provider：${id}`, 'info');
    await loadAll();
  } catch (err) {
    setNotice(`删除 Provider 失败：${String(err)}`, 'error');
  }
}

async function probeProvider(cfg: ProviderConfig) {
  probingMap[cfg.id] = true;
  try {
    // Save latest edited URL / API Key / Model before testing connectivity
    await invokeStudio<ProviderConfig>('upsert_llm_provider', { config: cfg });
    const res = await invokeStudio<any>('test_llm_provider_connectivity', { providerId: cfg.id });
    if (res.ok) {
      const msg = `连通正常（TTFT: ${res.ttft_ms ?? res.total_ms}ms）`;
      probeStatusMap[cfg.id] = { ok: true, text: msg };
      setNotice(`Provider ${cfg.name} (${cfg.id}) ${msg}`, 'success');
    } else {
      const msg = `探测失败：${res.error || '未知错误'}`;
      probeStatusMap[cfg.id] = { ok: false, text: msg };
      setNotice(`Provider ${cfg.name} (${cfg.id}) ${msg}`, 'error');
    }
  } catch (err) {
    const msg = `探测异常：${String(err)}`;
    probeStatusMap[cfg.id] = { ok: false, text: msg };
    setNotice(`Provider ${cfg.name} (${cfg.id}) ${msg}`, 'error');
  } finally {
    probingMap[cfg.id] = false;
  }
}

async function saveRouter() {
  try {
    const parseChain = (text: string) =>
      text
        .split(/[,，\s]+/)
        .map((s) => s.trim())
        .filter(Boolean);
    routerConfig.value.fast_chain = parseChain(fastChainText.value);
    routerConfig.value.reasoning_chain = parseChain(reasoningChainText.value);
    await invokeStudio('save_router_config', { config: routerConfig.value });
    setNotice('已更新 Fast/Reasoning 路由链与熔断策略', 'success');
  } catch (err) {
    setNotice(`保存路由策略失败：${String(err)}`, 'error');
  }
}

async function handleIssueKey() {
  try {
    const issued = await invokeStudio<any>('issue_api_key', {
      req: {
        tenant_id: newTenantId.value,
        name: newKeyName.value,
        qpm_limit: newQpm.value,
        tpm_limit: newTpm.value,
        namespace: newTenantId.value,
      },
    });
    newlyIssuedKey.value = issued.raw_key;
    setNotice(`已签发新 API Key：${newKeyName.value}`, 'success');
    await loadAll();
  } catch (err) {
    setNotice(`签发 API Key 失败：${String(err)}`, 'error');
  }
}

async function handleRevokeKey(keyId: string) {
  try {
    await invokeStudio('revoke_api_key', { keyId });
    setNotice(`已吊销 API Key：${keyId}`, 'info');
    await loadAll();
  } catch (err) {
    setNotice(`吊销 API Key 失败：${String(err)}`, 'error');
  }
}

onMounted(loadAll);
</script>

<template>
  <section class="space-y-6">
    <header class="flex flex-wrap items-center justify-between gap-4">
      <div>
        <h2 class="text-xl font-semibold text-white">AI 大模型与开放 API 网关配置中心</h2>
        <p class="text-sm text-slate-400">
          配置云端/院内私有化 LLM API 地址、API Key 与模型名称，支持一键连通性测速、Fast/Reasoning 故障转移路由与多租户 API Key 管理
        </p>
      </div>
      <button
        @click="showAddForm = !showAddForm"
        class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium text-white"
      >
        {{ showAddForm ? '收起新增表单' : '+ 新增 AI 模型 API' }}
      </button>
    </header>

    <div
      v-if="notice"
      :class="{
        'bg-emerald-950/70 border-emerald-700 text-emerald-300': noticeType === 'success',
        'bg-rose-950/70 border-rose-700 text-rose-300': noticeType === 'error',
        'bg-slate-900 border-slate-700 text-sky-300': noticeType === 'info',
      }"
      class="p-3 rounded-lg border text-sm"
    >
      {{ notice }}
    </div>

    <!-- Add New AI Model Provider Form -->
    <div v-if="showAddForm" class="p-4 rounded-xl bg-slate-900 border border-sky-700/60 space-y-4 text-xs">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <h3 class="font-semibold text-white text-sm">新增 AI 模型 API 配置</h3>
        <div class="flex flex-wrap items-center gap-1.5">
          <span class="text-slate-400">快速填充：</span>
          <button
            v-for="preset in presets"
            :key="preset.id"
            type="button"
            @click="applyPreset(preset)"
            class="px-2 py-1 rounded bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700"
          >
            {{ preset.label }}
          </button>
        </div>
      </div>

      <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
        <div>
          <label class="block text-slate-400 mb-1">Provider ID（唯一标识）</label>
          <input
            v-model="newProvider.id"
            placeholder="例如：deepseek_prod"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">显示名称</label>
          <input
            v-model="newProvider.name"
            placeholder="例如：DeepSeek 生产环境"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">协议类型</label>
          <select
            v-model="newProvider.kind"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700"
          >
            <option v-for="k in providerKinds" :key="k.value" :value="k.value">{{ k.label }}</option>
          </select>
        </div>

        <div class="md:col-span-2">
          <label class="block text-slate-400 mb-1">API Base URL</label>
          <input
            v-model="newProvider.base_url"
            placeholder="https://api.deepseek.com/v1"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">模型名称 (Model)</label>
          <input
            v-model="newProvider.model"
            placeholder="deepseek-chat / qwen-plus / gpt-4o-mini"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
          />
        </div>

        <div class="md:col-span-2">
          <label class="block text-slate-400 mb-1">API Key</label>
          <div class="flex gap-2">
            <input
              :type="showNewApiKey ? 'text' : 'password'"
              v-model="newProvider.api_key"
              placeholder="sk-..."
              autocomplete="off"
              class="flex-1 px-3 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
            />
            <button
              type="button"
              @click="showNewApiKey = !showNewApiKey"
              class="px-2.5 py-1.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-300"
            >
              {{ showNewApiKey ? '隐藏' : '显示' }}
            </button>
          </div>
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div>
            <label class="block text-slate-400 mb-1">模型分层</label>
            <select
              v-model="newProvider.tier"
              class="w-full px-2 py-1.5 rounded bg-slate-950 border border-slate-700"
            >
              <option value="fast">fast (快思考)</option>
              <option value="reasoning">reasoning (慢思考)</option>
            </select>
          </div>
          <div>
            <label class="block text-slate-400 mb-1">超时 (ms)</label>
            <input
              type="number"
              v-model.number="newProvider.timeout_ms"
              class="w-full px-2 py-1.5 rounded bg-slate-950 border border-slate-700"
            />
          </div>
        </div>
      </div>

      <div class="flex flex-wrap items-center justify-between gap-3 pt-1">
        <div class="flex items-center gap-4 text-slate-300">
          <label class="flex items-center gap-1.5">
            <input type="checkbox" v-model="newProvider.enabled" /> 启用
          </label>
          <label class="flex items-center gap-1.5">
            <input type="checkbox" v-model="newProvider.is_intranet" /> 院内私有化直连（跳过外发脱敏限制）
          </label>
        </div>
        <div class="flex gap-2">
          <button
            type="button"
            @click="showAddForm = false"
            class="px-3 py-1.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-300"
          >
            取消
          </button>
          <button
            type="button"
            @click="handleAddProvider"
            class="px-4 py-1.5 rounded bg-emerald-600 hover:bg-emerald-500 font-medium text-white"
          >
            保存并添加
          </button>
        </div>
      </div>
    </div>

    <!-- Configured Providers -->
    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <div
        v-for="p in providers"
        :key="p.id"
        class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 text-xs"
      >
        <div class="flex flex-wrap justify-between items-center gap-2">
          <div class="flex items-center gap-2">
            <span class="font-semibold text-white text-sm">{{ p.name }}</span>
            <span class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-400 font-mono text-[11px]">{{ p.id }}</span>
          </div>
          <div class="flex items-center gap-2">
            <label class="flex items-center gap-1 text-slate-300">
              <input type="checkbox" v-model="p.enabled" /> 启用
            </label>
            <label class="flex items-center gap-1 text-slate-300">
              <input type="checkbox" v-model="p.is_intranet" /> 院内
            </label>
          </div>
        </div>

        <div class="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
          <div>
            <label class="block text-slate-400 mb-1">名称</label>
            <input
              v-model="p.name"
              class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700"
            />
          </div>
          <div class="grid grid-cols-2 gap-2">
            <div>
              <label class="block text-slate-400 mb-1">协议类型</label>
              <select
                v-model="p.kind"
                class="w-full px-2 py-1.5 rounded bg-slate-950 border border-slate-700"
              >
                <option v-for="k in providerKinds" :key="k.value" :value="k.value">{{ k.label }}</option>
              </select>
            </div>
            <div>
              <label class="block text-slate-400 mb-1">分层</label>
              <select
                v-model="p.tier"
                class="w-full px-2 py-1.5 rounded bg-slate-950 border border-slate-700"
              >
                <option value="fast">fast</option>
                <option value="reasoning">reasoning</option>
              </select>
            </div>
          </div>

          <div class="sm:col-span-2">
            <label class="block text-slate-400 mb-1">API Base URL</label>
            <input
              v-model="p.base_url"
              placeholder="https://api.deepseek.com/v1"
              class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
            />
          </div>

          <div class="sm:col-span-2">
            <label class="block text-slate-400 mb-1">API Key</label>
            <div class="flex gap-2">
              <input
                :type="showApiKeyMap[p.id] ? 'text' : 'password'"
                v-model="p.api_key"
                placeholder="请输入您的 AI 模型 API Key（如 sk-...）"
                autocomplete="off"
                class="flex-1 px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
              />
              <button
                type="button"
                @click="showApiKeyMap[p.id] = !showApiKeyMap[p.id]"
                class="px-2.5 py-1.5 rounded bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-300"
              >
                {{ showApiKeyMap[p.id] ? '隐藏' : '显示' }}
              </button>
            </div>
          </div>

          <div>
            <label class="block text-slate-400 mb-1">模型名称 (Model)</label>
            <input
              v-model="p.model"
              placeholder="deepseek-chat"
              class="w-full px-2.5 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
            />
          </div>
          <div class="grid grid-cols-2 gap-2">
            <div>
              <label class="block text-slate-400 mb-1">超时 (ms)</label>
              <input
                type="number"
                v-model.number="p.timeout_ms"
                class="w-full px-2 py-1.5 rounded bg-slate-950 border border-slate-700"
              />
            </div>
            <div>
              <label class="block text-slate-400 mb-1">优先级</label>
              <input
                type="number"
                v-model.number="p.priority"
                class="w-full px-2 py-1.5 rounded bg-slate-950 border border-slate-700"
              />
            </div>
          </div>
        </div>

        <div
          v-if="probeStatusMap[p.id]"
          :class="probeStatusMap[p.id].ok ? 'text-emerald-300 bg-emerald-950/60 border-emerald-800' : 'text-rose-300 bg-rose-950/60 border-rose-800'"
          class="px-2.5 py-1.5 rounded border font-mono text-[11px] break-all"
        >
          {{ probeStatusMap[p.id].text }}
        </div>

        <div class="flex flex-wrap items-center justify-between gap-2 pt-1">
          <div class="flex gap-2">
            <button
              @click="probeProvider(p)"
              :disabled="probingMap[p.id]"
              class="px-3 py-1.5 rounded bg-amber-600 hover:bg-amber-500 disabled:opacity-50 font-medium text-white"
            >
              {{ probingMap[p.id] ? '测速中...' : '保存并连通性测速' }}
            </button>
            <button
              @click="saveProvider(p)"
              class="px-3 py-1.5 rounded bg-sky-600 hover:bg-sky-500 font-medium text-white"
            >
              保存配置
            </button>
          </div>
          <button
            @click="removeProvider(p.id)"
            class="px-2.5 py-1.5 rounded bg-slate-800 hover:bg-rose-900/70 text-rose-300 border border-slate-700"
          >
            删除
          </button>
        </div>
      </div>
    </div>

    <!-- Router Strategy -->
    <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 text-xs">
      <h3 class="font-semibold text-white text-sm">Fast / Reasoning 路由链与熔断策略</h3>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div>
          <label class="block text-slate-400 mb-1">Fast 路由链（逗号分隔 Provider ID）</label>
          <input
            v-model="fastChainText"
            placeholder="deepseek_fast, ollama_local"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">Reasoning 路由链（逗号分隔 Provider ID）</label>
          <input
            v-model="reasoningChainText"
            placeholder="qwen_med_reasoning"
            class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700 font-mono"
          />
        </div>
      </div>
      <div class="flex flex-wrap gap-4 items-end">
        <div>
          <label class="block text-slate-400 mb-1">默认超时 (ms)</label>
          <input
            type="number"
            v-model.number="routerConfig.default_timeout_ms"
            class="px-3 py-1.5 rounded bg-slate-950 border border-slate-700 w-32"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">熔断连续失败阈值</label>
          <input
            type="number"
            v-model.number="routerConfig.circuit_breaker_failure_threshold"
            class="px-3 py-1.5 rounded bg-slate-950 border border-slate-700 w-28"
          />
        </div>
        <div>
          <label class="block text-slate-400 mb-1">熔断冷却时间 (s)</label>
          <input
            type="number"
            v-model.number="routerConfig.circuit_breaker_cooldown_secs"
            class="px-3 py-1.5 rounded bg-slate-950 border border-slate-700 w-28"
          />
        </div>
        <button
          @click="saveRouter"
          class="px-4 py-1.5 rounded bg-emerald-600 hover:bg-emerald-500 font-medium text-white"
        >
          保存路由策略
        </button>
      </div>
    </div>

    <!-- API Keys & Metrics -->
    <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 text-xs">
      <div class="flex justify-between items-center">
        <h3 class="font-semibold text-white text-sm">对外多租户 API Key 管理</h3>
        <span v-if="metrics" class="text-slate-400">
          24h 请求 {{ metrics.total_requests }} · 错误率 {{ (metrics.error_rate * 100).toFixed(1) }}%
        </span>
      </div>
      <div class="grid grid-cols-2 md:grid-cols-5 gap-2">
        <input v-model="newTenantId" placeholder="租户ID" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <input v-model="newKeyName" placeholder="应用名称" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <input type="number" v-model.number="newQpm" placeholder="QPM" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <input type="number" v-model.number="newTpm" placeholder="TPM" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <button @click="handleIssueKey" class="px-3 py-1.5 rounded bg-sky-600 hover:bg-sky-500 font-medium text-white">
          签发新 Key
        </button>
      </div>
      <div v-if="newlyIssuedKey" class="p-2 rounded bg-emerald-950 border border-emerald-700 font-mono text-emerald-300">
        新签发 API Key（仅展示一次）：{{ newlyIssuedKey }}
      </div>
      <div class="space-y-1.5">
        <div v-for="k in apiKeys" :key="k.key_id" class="flex justify-between items-center p-2 rounded bg-slate-950">
          <span>
            <strong>{{ k.name }}</strong> ({{ k.tenant_id }}) · {{ k.key_prefix }}*** · QPM {{ k.qpm_limit }} / TPM {{ k.tpm_limit }}
          </span>
          <button v-if="!k.revoked" @click="handleRevokeKey(k.key_id)" class="text-rose-400 hover:underline">吊销</button>
          <span v-else class="text-slate-500">已吊销</span>
        </div>
      </div>
    </div>
  </section>
</template>
