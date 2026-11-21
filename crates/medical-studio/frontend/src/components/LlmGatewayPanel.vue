<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';

const providers = ref<any[]>([]);
const routerConfig = ref<any>({ fast_chain: [], reasoning_chain: [], default_timeout_ms: 8000, circuit_breaker_failure_threshold: 3, circuit_breaker_cooldown_secs: 30 });
const apiKeys = ref<any[]>([]);
const metrics = ref<any>(null);
const probeNotice = ref('');
const newlyIssuedKey = ref('');

const newTenantId = ref('his_clinic_01');
const newKeyName = ref('院内导诊机');
const newQpm = ref(120);
const newTpm = ref(100000);

async function loadAll() {
  providers.value = await invokeStudio('list_llm_providers');
  routerConfig.value = await invokeStudio('get_router_config');
  apiKeys.value = await invokeStudio('list_api_keys');
  metrics.value = await invokeStudio('get_gateway_metrics', { windowHours: 24 });
}

async function saveProvider(cfg: any) {
  await invokeStudio('upsert_llm_provider', { config: cfg });
  probeNotice.value = `已保存 Provider: ${cfg.name}`;
  await loadAll();
}

async function probeProvider(id: string) {
  const res = await invokeStudio<any>('test_llm_provider_connectivity', { providerId: id });
  probeNotice.value = res.ok
    ? `Provider ${id} 连通正常（TTFT: ${res.ttft_ms}ms）`
    : `Provider ${id} 探测失败：${res.error}`;
}

async function saveRouter() {
  await invokeStudio('save_router_config', { config: routerConfig.value });
  probeNotice.value = '已更新 Fast/Reasoning 路由与熔断策略';
}

async function handleIssueKey() {
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
  await loadAll();
}

async function handleRevokeKey(keyId: string) {
  await invokeStudio('revoke_api_key', { keyId });
  await loadAll();
}

onMounted(loadAll);
</script>

<template>
  <section class="space-y-6">
    <header>
      <h2 class="text-xl font-semibold text-white">AI 大模型与开放 API 网关配置中心</h2>
      <p class="text-sm text-slate-400">统一管理云端/院内私有化 LLM Provider、Fast/Reasoning 故障转移路由、多租户 API Key 签发与用量统计</p>
    </header>

    <div v-if="probeNotice" class="p-3 rounded-lg bg-slate-900 border border-slate-700 text-sm text-sky-300">{{ probeNotice }}</div>

    <!-- Providers -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
      <div v-for="p in providers" :key="p.id" class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-2 text-xs">
        <div class="flex justify-between items-center">
          <span class="font-semibold text-white text-sm">{{ p.name }}</span>
          <span class="px-2 py-0.5 rounded bg-sky-900 text-sky-200">{{ p.tier }}</span>
        </div>
        <div class="text-slate-400 font-mono">{{ p.base_url }} · {{ p.model }}</div>
        <div class="flex gap-2 pt-1">
          <button @click="probeProvider(p.id)" class="px-3 py-1 rounded bg-amber-600 hover:bg-amber-500">连通性测速</button>
          <button @click="saveProvider(p)" class="px-3 py-1 rounded bg-sky-600 hover:bg-sky-500">保存配置</button>
        </div>
      </div>
    </div>

    <!-- Router Strategy -->
    <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 flex flex-wrap gap-4 items-end text-xs">
      <div>
        <label class="block text-slate-400 mb-1">默认超时 (ms)</label>
        <input type="number" v-model.number="routerConfig.default_timeout_ms" class="px-3 py-1.5 rounded bg-slate-950 border border-slate-700 w-32" />
      </div>
      <div>
        <label class="block text-slate-400 mb-1">熔断连续失败阈值</label>
        <input type="number" v-model.number="routerConfig.circuit_breaker_failure_threshold" class="px-3 py-1.5 rounded bg-slate-950 border border-slate-700 w-28" />
      </div>
      <button @click="saveRouter" class="px-4 py-1.5 rounded bg-emerald-600 hover:bg-emerald-500 font-medium">保存路由策略</button>
    </div>

    <!-- API Keys & Metrics -->
    <div class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3 text-xs">
      <div class="flex justify-between items-center">
        <h3 class="font-semibold text-white text-sm">对外多租户 API Key 管理</h3>
        <span v-if="metrics" class="text-slate-400">24h 请求 {{ metrics.total_requests }} · 错误率 {{ (metrics.error_rate * 100).toFixed(1) }}%</span>
      </div>
      <div class="grid grid-cols-2 md:grid-cols-5 gap-2">
        <input v-model="newTenantId" placeholder="租户ID" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <input v-model="newKeyName" placeholder="应用名称" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <input type="number" v-model.number="newQpm" placeholder="QPM" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <input type="number" v-model.number="newTpm" placeholder="TPM" class="px-2 py-1.5 rounded bg-slate-950 border border-slate-700" />
        <button @click="handleIssueKey" class="px-3 py-1.5 rounded bg-sky-600 hover:bg-sky-500 font-medium">签发新 Key</button>
      </div>
      <div v-if="newlyIssuedKey" class="p-2 rounded bg-emerald-950 border border-emerald-700 font-mono text-emerald-300">
        新签发 API Key（仅展示一次）：{{ newlyIssuedKey }}
      </div>
      <div class="space-y-1.5">
        <div v-for="k in apiKeys" :key="k.key_id" class="flex justify-between items-center p-2 rounded bg-slate-950">
          <span><strong>{{ k.name }}</strong> ({{ k.tenant_id }}) · {{ k.key_prefix }}*** · QPM {{ k.qpm_limit }} / TPM {{ k.tpm_limit }}</span>
          <button v-if="!k.revoked" @click="handleRevokeKey(k.key_id)" class="text-rose-400 hover:underline">吊销</button>
          <span v-else class="text-slate-500">已吊销</span>
        </div>
      </div>
    </div>
  </section>
</template>
