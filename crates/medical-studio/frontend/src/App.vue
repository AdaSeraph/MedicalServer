<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import KnowledgeImportPanel from './components/KnowledgeImportPanel.vue';
import PatientMaskingPanel from './components/PatientMaskingPanel.vue';
import EpidemicConsolePanel from './components/EpidemicConsolePanel.vue';
import DictionaryManagerPanel from './components/DictionaryManagerPanel.vue';
import EcsDebuggerPanel from './components/EcsDebuggerPanel.vue';
import LlmGatewayPanel from './components/LlmGatewayPanel.vue';
import DataExchangePanel from './components/DataExchangePanel.vue';
import PatientFollowUpPanel from './components/PatientFollowUpPanel.vue';
import ToastContainer from './components/ToastContainer.vue';
import { initTheme, useTheme } from './composables/useTheme';
import { invokeStudio } from './api/tauri';

type TabId = 'knowledge' | 'patients' | 'followup' | 'epidemic' | 'dictionary' | 'ecs' | 'gateway' | 'exchange';
type OperatorRole = 'admin' | 'editor' | 'clinician';

interface RuntimeStatus {
  storage_mode: string;
  tantivy_persistent: boolean;
  tantivy_dir: string | null;
  tantivy_docs: number;
  local_snapshot_path: string | null;
  embedded_server_running: boolean;
  embedding_real: boolean;
  rerank_real: boolean;
  server_url: string;
  server_connected: boolean;
  server_health: any;
  init_note: string | null;
}

const activeTab = ref<TabId>('knowledge');
const operatorRole = ref<OperatorRole>('admin');
const sidebarCollapsed = ref(false);
const { theme, toggleTheme } = useTheme();
const runtimeStatus = ref<RuntimeStatus | null>(null);
const checkingRuntime = ref(false);
const savingSnapshot = ref(false);
const saveNotice = ref('');

async function refreshRuntimeStatus() {
  checkingRuntime.value = true;
  try {
    runtimeStatus.value = await invokeStudio<RuntimeStatus>('get_studio_runtime_status', {
      serverUrl: null,
    });
  } catch {
    runtimeStatus.value = null;
  } finally {
    checkingRuntime.value = false;
  }
}

async function handleSaveSnapshotNow() {
  savingSnapshot.value = true;
  saveNotice.value = '';
  try {
    saveNotice.value = await invokeStudio<string>('save_local_snapshot_now');
    setTimeout(() => {
      saveNotice.value = '';
    }, 3500);
  } catch (e: any) {
    saveNotice.value = `保存失败：${e?.message || e}`;
  } finally {
    savingSnapshot.value = false;
  }
}

const roleAllowedTabs: Record<OperatorRole, TabId[]> = {
  admin: ['knowledge', 'patients', 'followup', 'epidemic', 'dictionary', 'ecs', 'gateway', 'exchange'],
  editor: ['knowledge', 'epidemic', 'dictionary', 'ecs', 'exchange'],
  clinician: ['followup', 'patients', 'ecs'],
};

const visibleNavItems = computed(() =>
  navItems.filter((item) => roleAllowedTabs[operatorRole.value].includes(item.id)),
);

function setOperatorRole(role: OperatorRole) {
  operatorRole.value = role;
  if (!roleAllowedTabs[role].includes(activeTab.value)) {
    activeTab.value = roleAllowedTabs[role][0];
  }
}

const navItems: Array<{
  id: TabId;
  short: string;
  label: string;
  desc: string;
  shortcut: string;
  icon: string;
}> = [
  {
    id: 'knowledge',
    short: '知识库',
    label: '知识库导入工坊',
    desc: '多格式批量解析与 AI 规范化',
    shortcut: '1',
    icon: 'M12 6.253v13m0-13C10.832 5.477 9.246 5 7.5 5S4.168 5.477 3 6.253v13C4.168 18.477 5.754 18 7.5 18s3.332.477 4.5 1.253m0-13C13.168 5.477 14.754 5 16.5 5c1.747 0 3.332.477 4.5 1.253v13C19.832 18.477 18.247 18 16.5 18c-1.746 0-3.332.477-4.5 1.253',
  },
  {
    id: 'patients',
    short: '脱敏',
    label: '病人数据与脱敏中心',
    desc: 'PII 防火墙与 L2/L3 匿名入库',
    shortcut: '2',
    icon: 'M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z',
  },
  {
    id: 'followup',
    short: '复诊',
    label: '复诊反馈与新病人问诊',
    desc: '按序列号复诊反馈、新病人建档与问诊实时更新',
    shortcut: '3',
    icon: 'M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z',
  },
  {
    id: 'epidemic',
    short: '流行病',
    label: '季节与流行病控制台',
    desc: '自定义地区/季节高发病与热更新',
    shortcut: '4',
    icon: 'M13 10V3L4 14h7v7l9-11h-7z',
  },
  {
    id: 'dictionary',
    short: '词典',
    label: '关键词与急诊红线词典',
    desc: '同义词映射与 120 红线沙盒',
    shortcut: '5',
    icon: 'M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z',
  },
  {
    id: 'ecs',
    short: 'ECS',
    label: 'ECS 分诊白盒调试台',
    desc: '模拟问诊与阶段耗时透视',
    shortcut: '6',
    icon: 'M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z',
  },
  {
    id: 'gateway',
    short: '网关',
    label: 'AI 大模型与网关配置',
    desc: 'Provider 路由、熔断与 API Key',
    shortcut: '7',
    icon: 'M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z',
  },
  {
    id: 'exchange',
    short: '共享',
    label: '数据导出与交流共享',
    desc: '打包导出与导入他人强化包',
    shortcut: '8',
    icon: 'M8 7h12m0 0l-4-4m4 4l-4 4m0 6H4m0 0l4 4m-4-4l4-4',
  },
];

const componentMap = {
  knowledge: KnowledgeImportPanel,
  patients: PatientMaskingPanel,
  followup: PatientFollowUpPanel,
  epidemic: EpidemicConsolePanel,
  dictionary: DictionaryManagerPanel,
  ecs: EcsDebuggerPanel,
  gateway: LlmGatewayPanel,
  exchange: DataExchangePanel,
} as const;

const currentComponent = computed(() => componentMap[activeTab.value]);

function handleGlobalKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey) {
    const idx = Number(e.key) - 1;
    if (idx >= 0 && idx < navItems.length) {
      e.preventDefault();
      activeTab.value = navItems[idx].id;
    }
  }
}

onMounted(() => {
  initTheme();
  refreshRuntimeStatus();
  window.addEventListener('keydown', handleGlobalKeydown);
});

onBeforeUnmount(() => {
  window.removeEventListener('keydown', handleGlobalKeydown);
});
</script>

<template>
  <div class="h-screen flex overflow-hidden bg-slate-950 text-slate-100">
    <!-- Sidebar -->
    <aside
      :class="sidebarCollapsed ? 'w-[72px]' : 'w-64'"
      class="bg-slate-900 border-r border-slate-800 flex flex-col shrink-0 transition-all duration-200"
    >
      <!-- Brand Header -->
      <div class="p-4 border-b border-slate-800 flex items-center justify-between gap-2">
        <div v-if="!sidebarCollapsed" class="flex items-center gap-2.5 min-w-0">
          <div class="w-8 h-8 rounded-lg bg-gradient-to-br from-teal-500 to-sky-600 flex items-center justify-center text-white font-bold text-sm shadow-md shadow-sky-900/40 shrink-0">
            M
          </div>
          <div class="min-w-0">
            <h1 class="text-base font-bold text-white truncate tracking-tight">MedicalStudio</h1>
            <p class="text-[11px] text-slate-400 truncate">智能医疗分诊工作台</p>
          </div>
        </div>
        <div v-else class="mx-auto w-8 h-8 rounded-lg bg-gradient-to-br from-teal-500 to-sky-600 flex items-center justify-center text-white font-bold text-sm">
          M
        </div>
        <button
          v-if="!sidebarCollapsed"
          @click="sidebarCollapsed = true"
          class="p-1.5 rounded-lg text-slate-400 hover:text-white hover:bg-slate-800 transition-colors"
          title="收起侧边栏"
        >
          <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M11 19l-7-7 7-7m8 14l-7-7 7-7" />
          </svg>
        </button>
      </div>

      <!-- Navigation -->
      <nav class="p-2.5 space-y-1 flex-1 overflow-y-auto">
        <button
          v-for="item in visibleNavItems"
          :key="item.id"
          @click="activeTab = item.id"
          :title="sidebarCollapsed ? `${item.label} (Ctrl+${item.shortcut})` : item.desc"
          :class="
            activeTab === item.id
              ? 'bg-sky-600 text-white shadow-sm shadow-sky-900/40'
              : 'text-slate-300 hover:bg-slate-800/90 hover:text-white'
          "
          class="w-full flex items-center gap-2.5 px-3 py-2.5 rounded-xl text-sm font-medium transition-all group"
        >
          <svg class="w-4 h-4 shrink-0 opacity-90" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.8">
            <path stroke-linecap="round" stroke-linejoin="round" :d="item.icon" />
          </svg>
          <span v-if="!sidebarCollapsed" class="flex-1 text-left truncate">{{ item.label }}</span>
        </button>
      </nav>

      <!-- Footer Controls -->
      <div class="p-3 border-t border-slate-800 space-y-2">
        <button
          v-if="sidebarCollapsed"
          @click="sidebarCollapsed = false"
          class="w-full flex justify-center p-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300"
          title="展开侧边栏"
        >
          <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M13 5l7 7-7 7M5 5l7 7-7 7" />
          </svg>
        </button>

        <button
          @click="toggleTheme"
          class="w-full flex items-center justify-center gap-2 px-3 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs text-slate-200 transition-colors"
          :title="theme === 'dark' ? '切换至医疗浅色主题' : '切换至深色运维主题'"
        >
          <svg v-if="theme === 'dark'" class="w-4 h-4 text-amber-300 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
          </svg>
          <svg v-else class="w-4 h-4 text-sky-600 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
            <path stroke-linecap="round" stroke-linejoin="round" d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
          </svg>
          <span v-if="!sidebarCollapsed">{{ theme === 'dark' ? '切换医疗浅色模式' : '切换深色运维模式' }}</span>
        </button>

        <div v-if="!sidebarCollapsed" class="space-y-1">
          <label class="block text-[11px] text-slate-400 px-1">当前工作角色 (RBAC)</label>
          <select
            :value="operatorRole"
            @change="setOperatorRole(($event.target as HTMLSelectElement).value as OperatorRole)"
            class="w-full px-2.5 py-1.5 rounded-lg bg-slate-800 border border-slate-700 text-xs text-slate-200"
          >
            <option value="clinician">临床医生（复诊/脱敏/调试）</option>
            <option value="editor">知识库编辑（知识/词典/流行病）</option>
            <option value="admin">运维管理员（全部权限）</option>
          </select>
        </div>

        <div v-if="!sidebarCollapsed" class="flex items-center justify-between text-[11px] text-slate-500 px-1 pt-0.5">
          <span>Tauri 2.0 · Rust</span>
          <span>Ctrl+1~8 切换</span>
        </div>
      </div>
    </aside>

    <!-- Main Workspace (KeepAlive preserves queue, preview & debug state across tab switches) -->
    <main class="flex-1 p-6 overflow-y-auto min-w-0">
      <div
        v-if="runtimeStatus"
        :class="
          runtimeStatus.storage_mode === 'postgres' || runtimeStatus.storage_mode === 'local_encrypted_disk'
            ? 'border-emerald-500/40 bg-emerald-950/30 text-emerald-200'
            : 'border-amber-500/40 bg-amber-950/30 text-amber-200'
        "
        class="mb-4 px-3.5 py-2 rounded-lg border text-xs flex flex-wrap items-center justify-between gap-2"
      >
        <div class="flex flex-wrap items-center gap-2">
          <span v-if="runtimeStatus.storage_mode === 'postgres'">
            🟢 已连接 PostgreSQL 数据库持久化（知识库 {{ runtimeStatus.tantivy_docs }} 条）
          </span>
          <span v-else-if="runtimeStatus.storage_mode === 'local_encrypted_disk'">
            🟢 本机一体化模式已就绪 · 数据自动加密落盘（知识库 {{ runtimeStatus.tantivy_docs }} 条 · 重启不丢失）
          </span>
          <span v-else>
            ⚠️ 当前运行于纯内存模式
          </span>
          <span class="opacity-80">·</span>
          <span v-if="runtimeStatus.embedded_server_running" class="text-emerald-300 font-medium">
            内置分诊服务已自动启动 ({{ runtimeStatus.server_url }})
          </span>
          <span v-else-if="runtimeStatus.server_connected" class="text-emerald-300 font-medium">
            已连接本机分诊服务 ({{ runtimeStatus.server_url }})
          </span>
          <span v-else class="text-amber-300">
            分诊服务未连接 ({{ runtimeStatus.server_url }})
          </span>
          <span v-if="saveNotice" class="text-teal-300 font-medium">· {{ saveNotice }}</span>
          <span v-if="runtimeStatus.init_note" class="text-amber-300">· {{ runtimeStatus.init_note }}</span>
        </div>
        <div class="flex items-center gap-1.5 shrink-0">
          <button
            v-if="runtimeStatus.local_snapshot_path"
            :disabled="savingSnapshot"
            @click="handleSaveSnapshotNow"
            class="px-2 py-0.5 rounded bg-emerald-700/80 hover:bg-emerald-600 text-white text-[11px]"
          >
            {{ savingSnapshot ? '保存中...' : '立即保存' }}
          </button>
          <button
            :disabled="checkingRuntime"
            @click="refreshRuntimeStatus"
            class="px-2 py-0.5 rounded bg-slate-800/80 hover:bg-slate-700 text-slate-200 text-[11px]"
          >
            {{ checkingRuntime ? '检测中...' : '刷新状态' }}
          </button>
        </div>
      </div>
      <KeepAlive>
        <component :is="currentComponent" />
      </KeepAlive>
    </main>

    <!-- Global Toast Notifications -->
    <ToastContainer />
  </div>
</template>
