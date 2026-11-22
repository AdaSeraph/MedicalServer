<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';

const toast = useToast();

interface DepartmentCat {
  department: string;
  doc_count: number;
  diseases: string[];
  section_types: string[];
}

interface KnowledgeCategories {
  total_documents: number;
  departments: DepartmentCat[];
}

interface EcsStatus {
  using_real_llm: boolean;
  active_providers: string[];
  knowledge_doc_count: number;
  patient_count: number;
  epidemic_count: number;
  synonym_count: number;
  red_flag_count: number;
}

interface LlmProviderItem {
  id: string;
  name: string;
  model: string;
  enabled: boolean;
}

const activeTab = ref<'export' | 'import'>('export');
const isBusy = ref(false);

// Current store summary & categories
const status = ref<EcsStatus | null>(null);
const categories = ref<KnowledgeCategories | null>(null);
const providers = ref<LlmProviderItem[]>([]);
const selectedProviderId = ref('');

// Export options
const exportDescription = ref('MedicalStudio 学习交流共享包');
const includeKnowledge = ref(true);
const selectedDepartments = ref<string[]>([]);
const includePatients = ref(true);
const includeEpidemics = ref(true);
const includeDictionaries = ref(true);
const exportFormat = ref<'json' | 'knowledge_jsonl'>('json');
const tenantNamespace = ref('default');
const outputPath = ref('');
const lastExportResult = ref<any>(null);

// Import peer bundle options
const fileInputRef = ref<HTMLInputElement | null>(null);
const isDragging = ref(false);
const rawBundleText = ref('');
const bundleFileName = ref('');
const bundlePreview = ref<any>(null);
const importKnowledge = ref(true);
const importPatients = ref(true);
const importEpidemics = ref(true);
const importDictionaries = ref(true);
const inspectionResult = ref<any>(null);
const lastImportResult = ref<any>(null);

const inspectionByDocId = computed(() => {
  const map: Record<string, any> = {};
  for (const item of inspectionResult.value?.items ?? []) {
    map[item.doc_id] = item;
  }
  return map;
});

async function refreshSummary() {
  try {
    const [st, cats, provs] = await Promise.all([
      invokeStudio<EcsStatus>('get_ecs_runtime_status'),
      invokeStudio<KnowledgeCategories>('get_knowledge_categories', {
        tenantNamespace: tenantNamespace.value.trim() || null,
      }),
      invokeStudio<LlmProviderItem[]>('list_llm_providers'),
    ]);
    status.value = st;
    categories.value = cats;
    providers.value = provs ?? [];
    if (!selectedProviderId.value && providers.value.length > 0) {
      const first = providers.value.find((p) => p.enabled) ?? providers.value[0];
      selectedProviderId.value = first.id;
    }
  } catch (err) {
    toast.error(`加载数据统计失败：${String(err)}`);
  }
}

onMounted(refreshSummary);

function toggleDepartment(dept: string) {
  const idx = selectedDepartments.value.indexOf(dept);
  if (idx >= 0) {
    selectedDepartments.value.splice(idx, 1);
  } else {
    selectedDepartments.value.push(dept);
  }
}

function triggerBrowserDownload(filename: string, content: string, mime = 'application/json') {
  const blob = new Blob([content], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

async function handleExportPackage() {
  if (
    !includeKnowledge.value &&
    !includePatients.value &&
    !includeEpidemics.value &&
    !includeDictionaries.value
  ) {
    toast.error('请至少勾选一项要导出的数据内容');
    return;
  }
  isBusy.value = true;
  try {
    const res = await invokeStudio<any>('export_exchange_package', {
      req: {
        description: exportDescription.value.trim() || null,
        include_knowledge: includeKnowledge.value,
        knowledge_departments: selectedDepartments.value,
        tenant_namespace: tenantNamespace.value.trim() || null,
        include_patients: includePatients.value,
        include_epidemics: includeEpidemics.value,
        include_dictionaries: includeDictionaries.value,
        format: exportFormat.value,
        output_path: outputPath.value.trim() || null,
      },
    });
    lastExportResult.value = res;
    triggerBrowserDownload(
      res.filename,
      res.content,
      res.format === 'knowledge_jsonl' ? 'application/x-ndjson' : 'application/json',
    );
    const savedHint = res.saved_path ? `，并已写入本地 ${res.saved_path}` : '';
    toast.success(
      `导出成功：${res.filename}（知识 ${res.stats.knowledge_count} 条、脱敏病例 ${res.stats.patient_count} 份、流行病 ${res.stats.epidemic_count} 条、词典 ${res.stats.synonym_count + res.stats.red_flag_count} 条）${savedHint}`,
    );
  } catch (err) {
    toast.error(`导出失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

function triggerFileSelect() {
  fileInputRef.value?.click();
}

async function readExchangeFile(file: File) {
  bundleFileName.value = file.name;
  rawBundleText.value = await file.text();
  await handlePreviewBundle();
}

async function handleFileChange(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  if (!file) return;
  try {
    await readExchangeFile(file);
  } finally {
    input.value = '';
  }
}

async function handleDrop(event: DragEvent) {
  isDragging.value = false;
  const file = event.dataTransfer?.files?.[0];
  if (!file) return;
  await readExchangeFile(file);
}

async function handlePreviewBundle() {
  const text = rawBundleText.value.trim();
  if (!text) {
    toast.error('请先选择或粘贴交流共享包 JSON 内容');
    return;
  }
  isBusy.value = true;
  inspectionResult.value = null;
  lastImportResult.value = null;
  try {
    bundlePreview.value = await invokeStudio<any>('preview_exchange_package', {
      req: {
        content: text,
        tenant_namespace: tenantNamespace.value.trim() || null,
      },
    });
    const st = bundlePreview.value.stats;
    toast.success(
      `共享包解析通过：知识 ${st.knowledge_count} 条、病例 ${st.patient_count} 份、流行病 ${st.epidemic_count} 条、词典 ${st.synonym_count + st.red_flag_count} 条`,
    );
  } catch (err) {
    bundlePreview.value = null;
    toast.error(`共享包解析失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

async function handleAiInspectBundleKnowledge() {
  const docs = bundlePreview.value?.package?.knowledge_documents ?? [];
  if (docs.length === 0) {
    toast.info('该共享包中不包含知识库文档，无需知识巡查');
    return;
  }
  isBusy.value = true;
  try {
    const res = await invokeStudio<any>('ai_inspect_knowledge_batch', {
      req: {
        documents: docs,
        provider_id: selectedProviderId.value || null,
        tenant_namespace: tenantNamespace.value.trim() || null,
      },
    });
    inspectionResult.value = res;
    toast.success(
      `AI 巡查分类完成：通过 ${res.summary.pass_count}、需复核 ${res.summary.warning_count}、建议剔除 ${res.summary.reject_count}、疑似重复 ${res.summary.duplicate_count}`,
    );
  } catch (err) {
    toast.error(`AI 巡查分类失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}

function applyInspectionSuggestionsToBundle() {
  if (!bundlePreview.value?.package?.knowledge_documents || !inspectionResult.value) return;
  let updated = 0;
  for (const doc of bundlePreview.value.package.knowledge_documents) {
    const ins = inspectionByDocId.value[doc.doc_id];
    if (!ins) continue;
    if (ins.suggested_department && ins.suggested_department !== doc.department) {
      doc.department = ins.suggested_department;
      updated++;
    }
    if (ins.suggested_section_type) {
      doc.section_type = ins.suggested_section_type;
    }
    if (Array.isArray(ins.category_tags) && ins.category_tags.length > 0) {
      const merged = new Set([...(doc.seasonal_tags ?? []), ...ins.category_tags]);
      doc.seasonal_tags = Array.from(merged);
    }
  }
  toast.success(`已将 AI 分类建议应用到共享包中的知识条目（调整科室 ${updated} 条）`);
}

function removeFlaggedBundleDocs() {
  if (!bundlePreview.value?.package?.knowledge_documents || !inspectionResult.value) return;
  const before = bundlePreview.value.package.knowledge_documents.length;
  bundlePreview.value.package.knowledge_documents =
    bundlePreview.value.package.knowledge_documents.filter((doc: any) => {
      const ins = inspectionByDocId.value[doc.doc_id];
      if (!ins) return true;
      return ins.status !== 'reject' && !ins.is_duplicate;
    });
  const removed = before - bundlePreview.value.package.knowledge_documents.length;
  bundlePreview.value.stats.knowledge_count =
    bundlePreview.value.package.knowledge_documents.length;
  toast.info(`已剔除 ${removed} 条异常或重复知识条目`);
}

async function handleImportBundle() {
  if (!bundlePreview.value?.package) {
    toast.error('请先解析共享包后再执行导入');
    return;
  }
  isBusy.value = true;
  try {
    lastImportResult.value = await invokeStudio<any>('import_exchange_package', {
      req: {
        package: bundlePreview.value.package,
        tenant_namespace: tenantNamespace.value.trim() || 'default',
        import_knowledge: importKnowledge.value,
        import_patients: importPatients.value,
        import_epidemics: importEpidemics.value,
        import_dictionaries: importDictionaries.value,
      },
    });
    await refreshSummary();
    const r = lastImportResult.value;
    toast.success(
      `合并导入成功：知识 +${r.imported_knowledge}、病例 +${r.imported_patients}（L3事件 +${r.imported_l3_events}）、流行病 +${r.imported_epidemics}、词典 +${r.imported_synonyms + r.imported_red_flags}，ECS 已热更新`,
    );
  } catch (err) {
    toast.error(`合并导入失败：${String(err)}`);
  } finally {
    isBusy.value = false;
  }
}
</script>

<template>
  <section class="space-y-5">
    <header class="flex flex-wrap items-center justify-between gap-4">
      <div>
        <h2 class="text-xl font-semibold text-white">数据导出与交流共享中心</h2>
        <p class="text-sm text-slate-400">
          打包导出知识库、脱敏病例、流行病配置与词典规则为标准共享包，方便同程序用户互相交流学习与知识强化
        </p>
      </div>
      <div class="flex items-center gap-2">
        <button
          @click="refreshSummary"
          class="px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs text-slate-300"
        >
          刷新统计
        </button>
        <div class="flex rounded-lg bg-slate-900 p-1 border border-slate-800 text-xs">
          <button
            @click="activeTab = 'export'"
            :class="activeTab === 'export' ? 'bg-sky-600 text-white' : 'text-slate-400 hover:text-white'"
            class="px-3 py-1.5 rounded-md font-medium transition-colors"
          >
            打包导出共享包
          </button>
          <button
            @click="activeTab = 'import'"
            :class="activeTab === 'import' ? 'bg-emerald-600 text-white' : 'text-slate-400 hover:text-white'"
            class="px-3 py-1.5 rounded-md font-medium transition-colors"
          >
            导入他人共享包
          </button>
        </div>
      </div>
    </header>

    <!-- Current Local Asset Overview -->
    <div v-if="status" class="grid grid-cols-2 md:grid-cols-5 gap-3">
      <div class="p-3.5 rounded-xl bg-slate-900 border border-slate-800">
        <div class="text-xs text-slate-400">医学知识库</div>
        <div class="text-xl font-bold text-sky-400 mt-1">{{ status.knowledge_doc_count }} 条</div>
      </div>
      <div class="p-3.5 rounded-xl bg-slate-900 border border-slate-800">
        <div class="text-xs text-slate-400">脱敏病例库</div>
        <div class="text-xl font-bold text-emerald-400 mt-1">{{ status.patient_count }} 份</div>
      </div>
      <div class="p-3.5 rounded-xl bg-slate-900 border border-slate-800">
        <div class="text-xs text-slate-400">季节流行病配置</div>
        <div class="text-xl font-bold text-amber-400 mt-1">{{ status.epidemic_count }} 条</div>
      </div>
      <div class="p-3.5 rounded-xl bg-slate-900 border border-slate-800">
        <div class="text-xs text-slate-400">症状同义词</div>
        <div class="text-xl font-bold text-indigo-400 mt-1">{{ status.synonym_count }} 组</div>
      </div>
      <div class="p-3.5 rounded-xl bg-slate-900 border border-slate-800">
        <div class="text-xs text-slate-400">急诊 120 红线</div>
        <div class="text-xl font-bold text-rose-400 mt-1">{{ status.red_flag_count }} 条</div>
      </div>
    </div>

    <!-- Tab 1: Export -->
    <div v-if="activeTab === 'export'" class="bg-slate-900 p-5 rounded-xl border border-slate-800 space-y-5">
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div>
          <label class="block text-xs text-slate-400 mb-1">共享包说明（便于接收方了解知识范围与强化方向）</label>
          <input
            v-model="exportDescription"
            placeholder="例如：呼吸内科+急诊红线强化包 v1.0"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm text-slate-100"
          />
        </div>
        <div class="grid grid-cols-2 gap-3">
          <div>
            <label class="block text-xs text-slate-400 mb-1">导出格式</label>
            <select
              v-model="exportFormat"
              class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm text-slate-100"
            >
              <option value="json">完整交流共享包 (.json)</option>
              <option value="knowledge_jsonl">仅知识库流式文件 (.jsonl)</option>
            </select>
          </div>
          <div>
            <label class="block text-xs text-slate-400 mb-1">命名空间</label>
            <input
              v-model="tenantNamespace"
              class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm text-slate-100"
            />
          </div>
        </div>
      </div>

      <!-- Module Selection -->
      <div class="space-y-2">
        <div class="text-xs font-medium text-slate-300">选择要打包导出的模块（脱敏病例已移除明文 PII，可安全交流学习）：</div>
        <div class="flex flex-wrap gap-4 text-sm text-slate-200">
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" v-model="includeKnowledge" class="accent-sky-500" />
            <span>医学知识库 ({{ status?.knowledge_doc_count ?? 0 }})</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer" :class="exportFormat === 'knowledge_jsonl' ? 'opacity-40' : ''">
            <input type="checkbox" v-model="includePatients" :disabled="exportFormat === 'knowledge_jsonl'" class="accent-emerald-500" />
            <span>脱敏病例库 ({{ status?.patient_count ?? 0 }})</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer" :class="exportFormat === 'knowledge_jsonl' ? 'opacity-40' : ''">
            <input type="checkbox" v-model="includeEpidemics" :disabled="exportFormat === 'knowledge_jsonl'" class="accent-amber-500" />
            <span>流行病上下文 ({{ status?.epidemic_count ?? 0 }})</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer" :class="exportFormat === 'knowledge_jsonl' ? 'opacity-40' : ''">
            <input type="checkbox" v-model="includeDictionaries" :disabled="exportFormat === 'knowledge_jsonl'" class="accent-rose-500" />
            <span>同义词与红线词典 ({{ (status?.synonym_count ?? 0) + (status?.red_flag_count ?? 0) }})</span>
          </label>
        </div>
      </div>

      <!-- Department Filter -->
      <div v-if="includeKnowledge && categories && categories.departments.length > 0" class="space-y-2 pt-2 border-t border-slate-800">
        <div class="flex items-center justify-between text-xs">
          <span class="text-slate-400">
            按已有知识库分类筛选导出（不选则默认导出全部 {{ categories.departments.length }} 个科室）：
          </span>
          <button
            v-if="selectedDepartments.length > 0"
            @click="selectedDepartments = []"
            class="text-sky-400 hover:underline"
          >
            清空筛选（导出全部）
          </button>
        </div>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="cat in categories.departments"
            :key="cat.department"
            @click="toggleDepartment(cat.department)"
            :class="
              selectedDepartments.includes(cat.department)
                ? 'bg-sky-600 text-white border-sky-500'
                : 'bg-slate-950 text-slate-300 border-slate-700 hover:border-sky-600'
            "
            class="px-3 py-1 rounded-full border text-xs transition-colors"
          >
            {{ cat.department }} ({{ cat.doc_count }})
          </button>
        </div>
      </div>

      <!-- Optional Local Save Path -->
      <div class="pt-2 border-t border-slate-800">
        <label class="block text-xs text-slate-400 mb-1">
          可选：同时直接保存到本地磁盘路径（留空则仅通过浏览器下载保存）
        </label>
        <input
          v-model="outputPath"
          placeholder="例如：E:\MedicalExports\my_bundle.json"
          class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-xs font-mono text-slate-200"
        />
      </div>

      <div class="flex flex-wrap items-center gap-3">
        <button
          :disabled="isBusy"
          @click="handleExportPackage"
          class="px-5 py-2.5 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-sm font-medium text-white"
        >
          {{ isBusy ? '打包导出中...' : '生成并下载交流共享包' }}
        </button>
        <span v-if="lastExportResult" class="text-xs text-emerald-400">
          最近导出：{{ lastExportResult.filename }}（{{ (lastExportResult.content.length / 1024).toFixed(1) }} KB）
        </span>
      </div>
    </div>

    <!-- Tab 2: Import Peer Package -->
    <div v-else class="space-y-4">
      <div class="bg-slate-900 p-5 rounded-xl border border-slate-800 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 class="text-sm font-semibold text-white">1. 导入他人分享的交流包 (.json)</h3>
            <p class="text-xs text-slate-400">支持导入标准 DataCollectionPackage 共享包，自动校验格式并检测与本机知识库的重复项</p>
          </div>
          <div>
            <input
              ref="fileInputRef"
              type="file"
              accept=".json"
              class="hidden"
              @change="handleFileChange"
            />
            <button
              @click="triggerFileSelect"
              class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-sm font-medium text-white"
            >
              + 选择共享包文件 (.json)
            </button>
          </div>
        </div>

        <div
          @dragover.prevent="isDragging = true"
          @dragleave.prevent="isDragging = false"
          @drop.prevent="handleDrop"
          @click="triggerFileSelect"
          :class="isDragging ? 'border-emerald-400 bg-emerald-950/30' : 'border-slate-700 hover:border-slate-500 bg-slate-950/60'"
          class="border-2 border-dashed rounded-xl p-5 text-center cursor-pointer transition-colors"
        >
          <p class="text-sm text-slate-200">
            {{ bundleFileName ? `已载入：${bundleFileName}` : '点击此处或将对方导出的 .json 共享包拖拽到这里' }}
          </p>
        </div>

        <div class="space-y-2">
          <label class="text-xs text-slate-400">或直接粘贴共享包 JSON：</label>
          <textarea
            v-model="rawBundleText"
            rows="4"
            placeholder='{"$schema_version": "1.0.0", "knowledge_documents": [...], ...}'
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-xs font-mono text-slate-200"
          />
          <button
            :disabled="isBusy"
            @click="handlePreviewBundle"
            class="px-4 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs text-sky-300"
          >
            解析并预检共享包
          </button>
        </div>
      </div>

      <!-- Bundle Preview & Optional AI Inspection -->
      <div v-if="bundlePreview" class="bg-slate-900 p-5 rounded-xl border border-slate-800 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 class="text-sm font-semibold text-white">2. 共享包内容预检与 AI 巡查分类</h3>
            <p class="text-xs text-slate-400">
              {{ bundlePreview.description || '无说明' }} · Schema v{{ bundlePreview.schema_version }}
              <span v-if="bundlePreview.duplicate_knowledge_count > 0" class="text-amber-300 ml-2">
                （检出 {{ bundlePreview.duplicate_knowledge_count }} 条与本机疑似重复的知识）
              </span>
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <select
              v-model="selectedProviderId"
              class="px-2.5 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs text-slate-200"
            >
              <option v-for="p in providers" :key="p.id" :value="p.id">{{ p.name }} ({{ p.model }})</option>
            </select>
            <button
              :disabled="isBusy || bundlePreview.stats.knowledge_count === 0"
              @click="handleAiInspectBundleKnowledge"
              class="px-3.5 py-1.5 rounded-lg bg-amber-600 hover:bg-amber-500 disabled:opacity-40 text-xs font-medium text-white"
            >
              AI 巡查分类导入知识
            </button>
          </div>
        </div>

        <!-- Department breakdown in bundle -->
        <div v-if="bundlePreview.knowledge_departments?.length" class="flex flex-wrap gap-1.5 text-xs">
          <span class="text-slate-400 self-center">包含知识分类：</span>
          <span
            v-for="d in bundlePreview.knowledge_departments"
            :key="d.name"
            class="px-2.5 py-0.5 rounded-full bg-sky-950/70 border border-sky-700/60 text-sky-200"
          >
            {{ d.name }} · {{ d.count }} 条
          </span>
        </div>

        <!-- AI Inspection summary if executed -->
        <div v-if="inspectionResult" class="p-3.5 rounded-lg bg-slate-950 border border-amber-700/60 space-y-2 text-xs">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex flex-wrap items-center gap-3">
              <span class="font-semibold text-amber-300">AI 巡查分类结果：</span>
              <span class="text-emerald-400">通过 {{ inspectionResult.summary.pass_count }}</span>
              <span class="text-amber-300">需复核 {{ inspectionResult.summary.warning_count }}</span>
              <span class="text-rose-400">建议剔除 {{ inspectionResult.summary.reject_count }}</span>
              <span class="text-slate-400">疑似重复 {{ inspectionResult.summary.duplicate_count }}</span>
            </div>
            <div class="flex gap-2">
              <button
                @click="applyInspectionSuggestionsToBundle"
                class="px-2.5 py-1 rounded bg-sky-700 hover:bg-sky-600 text-white"
              >
                应用 AI 分类建议
              </button>
              <button
                @click="removeFlaggedBundleDocs"
                class="px-2.5 py-1 rounded bg-rose-700 hover:bg-rose-600 text-white"
              >
                剔除异常与重复项
              </button>
            </div>
          </div>
        </div>

        <!-- Selective Import Toggles -->
        <div class="flex flex-wrap items-center gap-4 text-sm text-slate-200 pt-2 border-t border-slate-800">
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" v-model="importKnowledge" class="accent-sky-500" />
            <span>合并知识库 ({{ bundlePreview.stats.knowledge_count }})</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" v-model="importPatients" class="accent-emerald-500" />
            <span>合并脱敏病例 ({{ bundlePreview.stats.patient_count }})</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" v-model="importEpidemics" class="accent-amber-500" />
            <span>合并流行病 ({{ bundlePreview.stats.epidemic_count }})</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" v-model="importDictionaries" class="accent-rose-500" />
            <span>合并词典与红线 ({{ bundlePreview.stats.synonym_count + bundlePreview.stats.red_flag_count }})</span>
          </label>
        </div>

        <button
          :disabled="isBusy"
          @click="handleImportBundle"
          class="px-5 py-2.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-sm font-medium text-white"
        >
          {{ isBusy ? '合并导入中...' : '确认合并到本地数据库并热更新分诊引擎' }}
        </button>
      </div>
    </div>
  </section>
</template>
