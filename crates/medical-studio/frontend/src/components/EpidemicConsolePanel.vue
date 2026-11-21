<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';

const contexts = ref<any[]>([]);
const serverUrl = ref('http://127.0.0.1:8080');
const adminToken = ref('e2e-admin-secret');
const notice = ref('');

async function loadContexts() {
  try {
    contexts.value = await invokeStudio<any[]>('list_epidemic_contexts');
  } catch (err) {
    notice.value = `加载流行病配置失败：${String(err)}`;
  }
}

async function saveContext(ctx: any) {
  try {
    await invokeStudio('upsert_epidemic_context', { ctx });
    notice.value = `已保存 ${ctx.disease_name}（强度 ${ctx.intensity}）`;
  } catch (err) {
    notice.value = `保存失败：${String(err)}`;
  }
}

async function triggerHotReload() {
  try {
    const res = await invokeStudio<any>('notify_server_hot_reload', {
      serverUrl: serverUrl.value,
      adminToken: adminToken.value,
    });
    notice.value = res.message;
  } catch (err) {
    notice.value = `热更新推送失败：${String(err)}`;
  }
}

onMounted(loadContexts);
</script>

<template>
  <section class="space-y-6">
    <header class="flex flex-wrap items-center justify-between gap-4">
      <div>
        <h2 class="text-xl font-semibold text-white">季节与流行病上下文控制台</h2>
        <p class="text-sm text-slate-400">动态调节地区流行病强度（0.0 ~ 1.0）与优先追问提示，秒级推送至 medical-server 热更新</p>
      </div>
      <div class="flex gap-2">
        <input v-model="serverUrl" placeholder="medical-server URL" class="px-3 py-2 rounded-lg bg-slate-900 border border-slate-700 text-xs" />
        <button @click="triggerHotReload" class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-sm font-medium">推送热更新</button>
      </div>
    </header>

    <div v-if="notice" class="p-3 rounded-lg bg-slate-900 border border-slate-700 text-sm text-sky-300">{{ notice }}</div>

    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
      <div v-for="ctx in contexts" :key="ctx.context_id" class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-3">
        <div class="flex items-center justify-between">
          <div>
            <h3 class="font-semibold text-white">{{ ctx.disease_name }}</h3>
            <p class="text-xs text-slate-400">{{ ctx.region_name }} ({{ ctx.region_code }}) · {{ ctx.season }} · #{{ ctx.seasonal_tag }}</p>
          </div>
          <label class="flex items-center gap-2 text-xs">
            <input type="checkbox" v-model="ctx.active" /> 启用
          </label>
        </div>

        <div>
          <div class="flex justify-between text-xs mb-1">
            <span class="text-slate-400">流行强度权重</span>
            <span class="font-mono text-amber-300">{{ Number(ctx.intensity).toFixed(2) }}</span>
          </div>
          <input type="range" min="0" max="1" step="0.05" v-model.number="ctx.intensity" class="w-full" />
        </div>

        <div>
          <label class="block text-xs text-slate-400 mb-1">优先追问引导语</label>
          <input v-model="ctx.priority_question_hint" class="w-full px-3 py-1.5 rounded bg-slate-950 border border-slate-700 text-xs" />
        </div>

        <button @click="saveContext(ctx)" class="px-3 py-1.5 rounded-lg bg-sky-600 hover:bg-sky-500 text-xs font-medium">保存配置</button>
      </div>
    </div>
  </section>
</template>
