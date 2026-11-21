<script setup lang="ts">
import { ref } from 'vue';
import KnowledgeImportPanel from './components/KnowledgeImportPanel.vue';
import PatientMaskingPanel from './components/PatientMaskingPanel.vue';
import EpidemicConsolePanel from './components/EpidemicConsolePanel.vue';
import DictionaryManagerPanel from './components/DictionaryManagerPanel.vue';
import EcsDebuggerPanel from './components/EcsDebuggerPanel.vue';
import LlmGatewayPanel from './components/LlmGatewayPanel.vue';

const activeTab = ref<'knowledge' | 'patients' | 'epidemic' | 'dictionary' | 'ecs' | 'gateway'>('knowledge');

const navItems = [
  { id: 'knowledge', label: '1. 知识库导入工坊' },
  { id: 'patients', label: '2. 病人数据与脱敏中心' },
  { id: 'epidemic', label: '3. 季节与流行病控制台' },
  { id: 'dictionary', label: '4. 关键词与急诊红线词典' },
  { id: 'ecs', label: '5. ECS 分诊白盒调试台' },
  { id: 'gateway', label: '6. AI 大模型与网关配置' },
] as const;
</script>

<template>
  <div class="min-h-screen flex bg-slate-950 text-slate-100">
    <aside class="w-64 bg-slate-900 border-r border-slate-800 p-4 flex flex-col">
      <div class="mb-6">
        <h1 class="text-lg font-bold text-sky-400">MedicalStudio</h1>
        <p class="text-xs text-slate-400">智能医疗分诊与知识库工作台</p>
      </div>
      <nav class="space-y-1.5 flex-1">
        <button
          v-for="item in navItems"
          :key="item.id"
          @click="activeTab = item.id"
          :class="activeTab === item.id ? 'bg-sky-600 text-white' : 'text-slate-300 hover:bg-slate-800'"
          class="w-full text-left px-3 py-2.5 rounded-lg text-sm font-medium transition-colors"
        >
          {{ item.label }}
        </button>
      </nav>
      <div class="text-[11px] text-slate-500 pt-4 border-t border-slate-800">
        Tauri 2.0 · Rust Workspace
      </div>
    </aside>

    <main class="flex-1 p-6 overflow-y-auto">
      <KnowledgeImportPanel v-if="activeTab === 'knowledge'" />
      <PatientMaskingPanel v-else-if="activeTab === 'patients'" />
      <EpidemicConsolePanel v-else-if="activeTab === 'epidemic'" />
      <DictionaryManagerPanel v-else-if="activeTab === 'dictionary'" />
      <EcsDebuggerPanel v-else-if="activeTab === 'ecs'" />
      <LlmGatewayPanel v-else />
    </main>
  </div>
</template>
