<script setup lang="ts">
import { ref } from 'vue';
import { invokeStudio } from '../api/tauri';

const filePath = ref('e2e/fixtures/knowledge/books_sample.md');
const jobId = ref('job_kb_001');
const tenantNamespace = ref('default');
const diseaseRegex = ref('^【疾病】\\s*(?P<disease>.+)$');
const sectionRegex = ref('^【(?P<section>临床表现|鉴别诊断|急症指征|推荐科室)】\\s*(?P<body>.*)$');
const errorOutputPath = ref('import_errors.jsonl');

const previewChunks = ref<any[]>([]);
const previewErrors = ref<string[]>([]);
const jobProgress = ref<any>(null);
const statusMessage = ref('');

async function handlePreview() {
  try {
    const res = await invokeStudio<any>('preview_knowledge_chunks', {
      req: {
        file_path: filePath.value,
        custom_rule: {
          disease_pattern: diseaseRegex.value,
          section_pattern: sectionRegex.value,
        },
        limit: 20,
      },
    });
    previewChunks.value = res.preview_chunks ?? [];
    previewErrors.value = res.errors ?? [];
    statusMessage.value = `切片预览完成：共 ${res.total_chunks} 个医学逻辑块`;
  } catch (err) {
    statusMessage.value = `预览失败：${String(err)}`;
  }
}

async function handleStartImport() {
  try {
    jobProgress.value = await invokeStudio<any>('start_knowledge_import', {
      req: {
        job_id: jobId.value,
        file_path: filePath.value,
        tenant_namespace: tenantNamespace.value,
      },
    });
    statusMessage.value = `导入完成：已处理 ${jobProgress.value.processed_items} 条`;
  } catch (err) {
    statusMessage.value = `导入失败：${String(err)}`;
  }
}

async function handleResumeImport() {
  try {
    jobProgress.value = await invokeStudio<any>('resume_knowledge_import', {
      jobId: jobId.value,
      tenantNamespace: tenantNamespace.value,
    });
    statusMessage.value = `断点续传已执行，状态：${jobProgress.value.status}`;
  } catch (err) {
    statusMessage.value = `续传失败：${String(err)}`;
  }
}

async function handleRefreshStatus() {
  try {
    const cp = await invokeStudio<any>('get_import_job_status', { jobId: jobId.value });
    jobProgress.value = cp;
    statusMessage.value = cp ? `任务状态：${cp.status}` : '未找到该任务检查点';
  } catch (err) {
    statusMessage.value = `查询状态失败：${String(err)}`;
  }
}

async function handleExportErrors() {
  try {
    const count = await invokeStudio<number>('export_import_errors', {
      jobId: jobId.value,
      outputPath: errorOutputPath.value,
    });
    statusMessage.value = `已导出 ${count} 条错误日志至 ${errorOutputPath.value}`;
  } catch (err) {
    statusMessage.value = `导出失败：${String(err)}`;
  }
}
</script>

<template>
  <section class="space-y-6">
    <header>
      <h2 class="text-xl font-semibold text-white">医学知识库与书籍导入工坊</h2>
      <p class="text-sm text-slate-400">支持 JSON/JSONL、Markdown、TXT、DOCX、PDF 语义切块、元数据注入、双写索引与断点续传</p>
    </header>

    <div class="grid grid-cols-1 md:grid-cols-3 gap-4 bg-slate-900 p-4 rounded-xl border border-slate-800">
      <div class="md:col-span-2">
        <label class="block text-xs text-slate-400 mb-1">源文件 / 知识包路径</label>
        <input v-model="filePath" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
      </div>
      <div>
        <label class="block text-xs text-slate-400 mb-1">任务 ID / 租户命名空间</label>
        <div class="flex gap-2">
          <input v-model="jobId" class="w-1/2 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
          <input v-model="tenantNamespace" class="w-1/2 px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
        </div>
      </div>
      <div class="md:col-span-3 flex flex-wrap gap-2">
        <button @click="handlePreview" class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium">切片预览</button>
        <button @click="handleStartImport" class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-sm font-medium">开始双写入库</button>
        <button @click="handleResumeImport" class="px-4 py-2 rounded-lg bg-amber-600 hover:bg-amber-500 text-sm font-medium">断点续传</button>
        <button @click="handleRefreshStatus" class="px-4 py-2 rounded-lg bg-slate-700 hover:bg-slate-600 text-sm">查询检查点</button>
        <button @click="handleExportErrors" class="px-4 py-2 rounded-lg bg-rose-700 hover:bg-rose-600 text-sm">导出错误日志</button>
      </div>
    </div>

    <div v-if="statusMessage" class="p-3 rounded-lg bg-slate-900 border border-slate-700 text-sm text-sky-300">
      {{ statusMessage }}
    </div>

    <div v-if="jobProgress" class="p-4 rounded-xl bg-slate-900 border border-slate-800 grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
      <div><span class="text-slate-400">状态：</span><span class="font-semibold text-emerald-400">{{ jobProgress.status }}</span></div>
      <div><span class="text-slate-400">已完成：</span>{{ jobProgress.processed_items }} / {{ jobProgress.total_items }}</div>
      <div><span class="text-slate-400">失败：</span>{{ jobProgress.failed_items }}</div>
      <div><span class="text-slate-400">偏移量：</span>{{ jobProgress.byte_offset }} B</div>
    </div>

    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
      <article v-for="chunk in previewChunks" :key="chunk.doc_id" class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-2">
        <div class="flex items-center justify-between">
          <h3 class="font-semibold text-white">{{ chunk.disease_name }}</h3>
          <span class="text-xs px-2 py-0.5 rounded bg-sky-900 text-sky-200">{{ chunk.department }}</span>
        </div>
        <div class="flex flex-wrap gap-1.5 text-xs">
          <span class="px-2 py-0.5 rounded bg-slate-800 text-slate-300">{{ chunk.section_type }}</span>
          <span class="px-2 py-0.5 rounded bg-slate-800 text-emerald-300">权威度 {{ chunk.authority_level }}</span>
          <span v-for="tag in chunk.seasonal_tags" :key="tag" class="px-2 py-0.5 rounded bg-amber-900/60 text-amber-200">#{{ tag }}</span>
        </div>
        <p class="text-xs text-slate-300 leading-relaxed">{{ chunk.content }}</p>
      </article>
    </div>
  </section>
</template>
