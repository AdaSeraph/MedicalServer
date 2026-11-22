<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';
import {
  formatBytes,
  readBrowserFilesToQueue,
  type BatchImportItem,
  type QueuedImportFile,
} from '../utils/batchFiles';
import ClinicalMediaDropzone, { type LocalClinicalMediaItem } from './ClinicalMediaDropzone.vue';

const toast = useToast();

interface LlmProviderItem {
  id: string;
  name: string;
  model: string;
  enabled: boolean;
}

interface DepartmentCategory {
  department: string;
  doc_count: number;
  diseases: string[];
  section_types: string[];
}

interface CategoryCountItem {
  name: string;
  count: number;
}

interface KnowledgeCategoriesData {
  total_documents: number;
  departments: DepartmentCategory[];
  section_types: CategoryCountItem[];
  source_types: CategoryCountItem[];
  seasonal_tags: CategoryCountItem[];
  populations: CategoryCountItem[];
}

const activeMode = ref<'batch_ai' | 'local_path'>('batch_ai');

// Existing KB categories overview
const kbCategories = ref<KnowledgeCategoriesData | null>(null);
const selectedBrowseDept = ref<string | null>(null);
const browseDocs = ref<any[]>([]);
const isLoadingCategories = ref(false);

// Batch & AI normalization state
const fileInputRef = ref<HTMLInputElement | null>(null);
const isDragging = ref(false);
const queuedFiles = ref<QueuedImportFile[]>([]);
const kbAttachedMedia = ref<LocalClinicalMediaItem[]>([]);
const pastedText = ref('');
const pastedLabel = ref('自由粘贴医学资料.txt');

const providers = ref<LlmProviderItem[]>([]);
const selectedProviderId = ref('');
const customPrompt = ref('');
const defaultPrompt = ref('');
const showPromptEditor = ref(false);

const sourceTitle = ref('医学知识库批量导入');
const defaultDepartment = ref('全科医学科');
const authorityLevel = ref(0.9);

const jobId = ref('job_kb_001');
const tenantNamespace = ref('default');

const isBusy = ref(false);
const isInspecting = ref(false);
const normalizedDocs = ref<any[]>([]);
const itemResults = ref<any[]>([]);
const normalizeErrors = ref<string[]>([]);
const lastAiMeta = ref<{ provider: string; model: string; tokens: number } | null>(null);
const inspectionResult = ref<any>(null);

const inspectionByDocId = computed(() => {
  const map: Record<string, any> = {};
  for (const item of inspectionResult.value?.items ?? []) {
    map[item.doc_id] = item;
  }
  return map;
});

const sectionTypeLabel: Record<string, string> = {
  clinical_manifestation: '临床表现',
  differential_diagnosis: '鉴别诊断',
  red_flag: '急症红线',
  triage_rule: '分诊规则',
};

// Local path / resumable mode state
const filePath = ref('e2e/fixtures/knowledge/books_sample.md');
const diseaseRegex = ref('^【疾病】\\s*(?P<disease>.+)$');
const sectionRegex = ref('^【(?P<section>临床表现|鉴别诊断|急症指征|推荐科室)】\\s*(?P<body>.*)$');
const errorOutputPath = ref('import_errors.jsonl');

const jobProgress = ref<any>(null);
const statusMessage = ref('');
const statusKind = ref<'info' | 'success' | 'error'>('info');

function setStatus(msg: string, kind: 'info' | 'success' | 'error' = 'info') {
  statusMessage.value = msg;
  statusKind.value = kind;
  if (kind !== 'info') {
    toast.push(msg, kind);
  }
}

async function loadInitialConfig() {
  try {
    const [list, prompts] = await Promise.all([
      invokeStudio<LlmProviderItem[]>('list_llm_providers'),
      invokeStudio<{ knowledge_prompt: string; patient_prompt: string }>('get_ai_import_prompts'),
    ]);
    providers.value = list ?? [];
    if (!selectedProviderId.value && providers.value.length > 0) {
      const firstEnabled = providers.value.find((p) => p.enabled) ?? providers.value[0];
      selectedProviderId.value = firstEnabled.id;
    }
    defaultPrompt.value = prompts.knowledge_prompt;
    if (!customPrompt.value) {
      customPrompt.value = prompts.knowledge_prompt;
    }
  } catch (err) {
    setStatus(`加载 AI 模型或提示词失败：${String(err)}`, 'error');
  }
}

async function loadCategories() {
  isLoadingCategories.value = true;
  try {
    kbCategories.value = await invokeStudio<KnowledgeCategoriesData>('get_knowledge_categories', {
      tenantNamespace: tenantNamespace.value.trim() || null,
    });
    if (selectedBrowseDept.value) {
      await browseDepartment(selectedBrowseDept.value);
    }
  } catch (err) {
    setStatus(`加载已有知识库分类失败：${String(err)}`, 'error');
  } finally {
    isLoadingCategories.value = false;
  }
}

async function browseDepartment(dept: string) {
  if (selectedBrowseDept.value === dept && browseDocs.value.length > 0) {
    selectedBrowseDept.value = null;
    browseDocs.value = [];
    return;
  }
  selectedBrowseDept.value = dept;
  try {
    browseDocs.value = await invokeStudio<any[]>('list_knowledge_documents', {
      department: dept,
      tenantNamespace: tenantNamespace.value.trim() || null,
      limit: 50,
    });
  } catch (err) {
    toast.error(`加载科室「${dept}」文档失败：${String(err)}`);
  }
}

async function quickExportKnowledge(dept?: string | null) {
  try {
    const res = await invokeStudio<any>('export_exchange_package', {
      req: {
        description: dept
          ? `MedicalStudio 知识库分类导出：${dept}`
          : 'MedicalStudio 全部医学知识库导出',
        include_knowledge: true,
        knowledge_departments: dept ? [dept] : [],
        tenant_namespace: tenantNamespace.value.trim() || null,
        include_patients: false,
        include_epidemics: false,
        include_dictionaries: false,
        format: 'json',
      },
    });
    const blob = new Blob([res.content], { type: 'application/json;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = res.filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    toast.success(`已导出 ${res.stats.knowledge_count} 条知识至 ${res.filename}`);
  } catch (err) {
    toast.error(`导出知识库失败：${String(err)}`);
  }
}

async function handleAiInspectKnowledge() {
  if (normalizedDocs.value.length === 0) {
    setStatus('当前没有待巡查分类的知识条目，请先解析或规范化资料', 'error');
    return;
  }
  isInspecting.value = true;
  setStatus(`正在对 ${normalizedDocs.value.length} 条待入库知识执行 AI 巡查分类（自动对齐已有科室分类、检测重复与质量）...`, 'info');
  try {
    const res = await invokeStudio<any>('ai_inspect_knowledge_batch', {
      req: {
        documents: normalizedDocs.value,
        provider_id: selectedProviderId.value || null,
        tenant_namespace: tenantNamespace.value.trim() || null,
      },
    });
    inspectionResult.value = res;
    setStatus(
      `AI 巡查分类完成：通过 ${res.summary.pass_count}、需复核 ${res.summary.warning_count}、建议剔除 ${res.summary.reject_count}、疑似重复 ${res.summary.duplicate_count}（模型：${res.model_used}）`,
      res.summary.reject_count > 0 ? 'info' : 'success',
    );
  } catch (err) {
    setStatus(`AI 巡查分类失败：${String(err)}`, 'error');
  } finally {
    isInspecting.value = false;
  }
}

function applyAllAiClassifications() {
  if (!inspectionResult.value) return;
  let deptUpdated = 0;
  for (const doc of normalizedDocs.value) {
    const ins = inspectionByDocId.value[doc.doc_id];
    if (!ins) continue;
    if (ins.suggested_department && ins.suggested_department !== doc.department) {
      doc.department = ins.suggested_department;
      deptUpdated++;
    }
    if (ins.suggested_section_type) {
      doc.section_type = ins.suggested_section_type;
    }
    if (Array.isArray(ins.category_tags) && ins.category_tags.length > 0) {
      const merged = new Set([...(doc.seasonal_tags ?? []), ...ins.category_tags]);
      doc.seasonal_tags = Array.from(merged);
    }
  }
  setStatus(`已一键应用 AI 巡查分类建议（规范调整科室 ${deptUpdated} 条，并补充分类标签）`, 'success');
}

function removeRejectedAndDuplicates() {
  if (!inspectionResult.value) return;
  const before = normalizedDocs.value.length;
  normalizedDocs.value = normalizedDocs.value.filter((doc) => {
    const ins = inspectionByDocId.value[doc.doc_id];
    if (!ins) return true;
    return ins.status !== 'reject' && !ins.is_duplicate;
  });
  const removed = before - normalizedDocs.value.length;
  setStatus(`已剔除 ${removed} 条异常或重复条目，剩余 ${normalizedDocs.value.length} 条可安全入库`, 'info');
}

onMounted(() => {
  loadInitialConfig();
  loadCategories();
});

function triggerFileSelect() {
  fileInputRef.value?.click();
}

async function handleFileInputChange(event: Event) {
  const input = event.target as HTMLInputElement;
  if (!input.files || input.files.length === 0) return;
  try {
    const added = await readBrowserFilesToQueue(input.files);
    queuedFiles.value.push(...added);
    setStatus(`已添加 ${added.length} 个文件到批量导入队列（共 ${queuedFiles.value.length} 项）`, 'info');
  } catch (err) {
    setStatus(`读取文件失败：${String(err)}`, 'error');
  } finally {
    input.value = '';
  }
}

async function handleDrop(event: DragEvent) {
  isDragging.value = false;
  const files = event.dataTransfer?.files;
  if (!files || files.length === 0) return;
  try {
    const added = await readBrowserFilesToQueue(files);
    queuedFiles.value.push(...added);
    setStatus(`已拖入 ${added.length} 个文件到批量导入队列（共 ${queuedFiles.value.length} 项）`, 'info');
  } catch (err) {
    setStatus(`读取拖入文件失败：${String(err)}`, 'error');
  }
}

function handleAddPastedToQueue() {
  const text = pastedText.value.trim();
  if (!text) {
    setStatus('请先粘贴需要规范化的医学知识内容', 'error');
    return;
  }
  const label = pastedLabel.value.trim() || `粘贴文本_${queuedFiles.value.length + 1}.txt`;
  queuedFiles.value.push({
    id: `paste-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
    name: label,
    size: new Blob([text]).size,
    kind: 'paste',
    item: {
      name: label,
      content_text: text,
    },
  });
  pastedText.value = '';
  setStatus(`已将「${label}」加入批量导入队列（共 ${queuedFiles.value.length} 项）`, 'info');
}

function removeQueuedItem(id: string) {
  queuedFiles.value = queuedFiles.value.filter((f) => f.id !== id);
}

function clearQueue() {
  queuedFiles.value = [];
  normalizedDocs.value = [];
  itemResults.value = [];
  normalizeErrors.value = [];
  lastAiMeta.value = null;
}

function collectBatchItems(): BatchImportItem[] {
  const items: BatchImportItem[] = queuedFiles.value.map((q) => q.item);
  const inlineText = pastedText.value.trim();
  if (inlineText) {
    items.push({
      name: pastedLabel.value.trim() || '直接粘贴知识文本.txt',
      content_text: inlineText,
    });
  }
  return items;
}

function buildDefaultsPayload() {
  return {
    source_title: sourceTitle.value.trim() || '医学知识库批量导入',
    source_type: 'textbook',
    authority_level: Number(authorityLevel.value) || 0.9,
    default_department: defaultDepartment.value.trim() || '全科医学科',
    default_population: ['adult'],
    seasonal_tags: [],
  };
}

async function handleAiNormalizePreview(): Promise<boolean> {
  const items = collectBatchItems();
  if (items.length === 0) {
    setStatus('请先点击「导入文件」选择文件，或在下方粘贴原始文本', 'error');
    return false;
  }
  isBusy.value = true;
  setStatus(`正在调用 AI 模型批量规范化 ${items.length} 份资料，请稍候...`, 'info');
  try {
    const res = await invokeStudio<any>('ai_normalize_knowledge_batch', {
      req: {
        items,
        provider_id: selectedProviderId.value || null,
        custom_prompt: customPrompt.value.trim() || null,
        defaults: buildDefaultsPayload(),
      },
    });
    normalizedDocs.value = res.documents ?? [];
    itemResults.value = res.item_results ?? [];
    normalizeErrors.value = res.errors ?? [];
    lastAiMeta.value = {
      provider: res.provider_used,
      model: res.model_used,
      tokens: res.total_tokens ?? 0,
    };
    if (normalizedDocs.value.length > 0) {
      setStatus(
        `AI 规范化完成：共生成 ${normalizedDocs.value.length} 条标准医学知识（模型：${res.model_used}，Tokens：${res.total_tokens}），核对后点击「确认写入数据库」`,
        'success',
      );
      return true;
    } else {
      setStatus(`AI 规范化未生成有效条目，请查看下方错误日志或检查模型 API Key 配置`, 'error');
      return false;
    }
  } catch (err) {
    setStatus(`AI 规范化失败：${String(err)}`, 'error');
    return false;
  } finally {
    isBusy.value = false;
  }
}

async function handleRuleBatchPreview() {
  const items = collectBatchItems();
  if (items.length === 0) {
    setStatus('请先添加至少一个文件或文本', 'error');
    return;
  }
  isBusy.value = true;
  try {
    const res = await invokeStudio<any>('batch_preview_knowledge_items', {
      req: {
        items,
        defaults: buildDefaultsPayload(),
        custom_rule: {
          disease_pattern: diseaseRegex.value,
          section_pattern: sectionRegex.value,
        },
      },
    });
    normalizedDocs.value = res.preview_chunks ?? [];
    normalizeErrors.value = res.errors ?? [];
    itemResults.value = [];
    lastAiMeta.value = null;
    setStatus(`规则批量解析完成：共 ${res.total_chunks} 条标准知识块，可直接写入数据库`, 'success');
  } catch (err) {
    setStatus(`规则批量解析失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function handleCommitNormalizedDocs() {
  if (normalizedDocs.value.length === 0) {
    setStatus('当前没有待写入的标准知识条目，请先执行规范化或解析', 'error');
    return;
  }
  isBusy.value = true;
  try {
    const batchJobId = jobId.value.trim() || `job_kb_batch_${Date.now()}`;
    const strippedMedia = kbAttachedMedia.value.map(({ previewUrl: _p, ...rest }) => rest);
    const docsToCommit = normalizedDocs.value.map((d) =>
      strippedMedia.length > 0
        ? { ...d, media_assets: [...(d.media_assets || []), ...strippedMedia] }
        : d,
    );
    jobProgress.value = await invokeStudio<any>('commit_knowledge_documents', {
      req: {
        job_id: batchJobId,
        tenant_namespace: tenantNamespace.value.trim() || 'default',
        documents: docsToCommit,
      },
    });
    kbAttachedMedia.value = [];
    setStatus(
      `批量入库成功：已双写 ${jobProgress.value.processed_items} 条标准知识到向量库与全文索引（任务 ${batchJobId}）`,
      'success',
    );
    inspectionResult.value = null;
    await loadCategories();
  } catch (err) {
    setStatus(`写入数据库失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function handleOneClickAiImport() {
  const ok = await handleAiNormalizePreview();
  if (ok && normalizedDocs.value.length > 0) {
    await handleCommitNormalizedDocs();
  }
}

function removeNormalizedDoc(docId: string) {
  normalizedDocs.value = normalizedDocs.value.filter((d) => d.doc_id !== docId);
}

// Local path / resumable handlers (preserved)
async function handlePreview() {
  try {
    const res = await invokeStudio<any>('preview_knowledge_chunks', {
      req: {
        file_path: filePath.value,
        custom_rule: {
          disease_pattern: diseaseRegex.value,
          section_pattern: sectionRegex.value,
        },
        limit: 50,
      },
    });
    normalizedDocs.value = res.preview_chunks ?? [];
    normalizeErrors.value = res.errors ?? [];
    setStatus(`切片预览完成：共 ${res.total_chunks} 个医学逻辑块`, 'success');
  } catch (err) {
    setStatus(`预览失败：${String(err)}`, 'error');
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
    setStatus(`导入完成：已处理 ${jobProgress.value.processed_items} 条`, 'success');
    await loadCategories();
  } catch (err) {
    setStatus(`导入失败：${String(err)}`, 'error');
  }
}

async function handleResumeImport() {
  try {
    jobProgress.value = await invokeStudio<any>('resume_knowledge_import', {
      jobId: jobId.value,
      tenantNamespace: tenantNamespace.value,
    });
    setStatus(`断点续传已执行，状态：${jobProgress.value.status}`, 'success');
  } catch (err) {
    setStatus(`续传失败：${String(err)}`, 'error');
  }
}

async function handleRefreshStatus() {
  try {
    const cp = await invokeStudio<any>('get_import_job_status', { jobId: jobId.value });
    jobProgress.value = cp;
    setStatus(cp ? `任务状态：${cp.status}` : '未找到该任务检查点', 'info');
  } catch (err) {
    setStatus(`查询状态失败：${String(err)}`, 'error');
  }
}

async function handleExportErrors() {
  try {
    const count = await invokeStudio<number>('export_import_errors', {
      jobId: jobId.value,
      outputPath: errorOutputPath.value,
    });
    setStatus(`已导出 ${count} 条错误日志至 ${errorOutputPath.value}`, 'info');
  } catch (err) {
    setStatus(`导出失败：${String(err)}`, 'error');
  }
}
</script>

<template>
  <section class="space-y-6">
    <header class="flex flex-wrap items-center justify-between gap-4">
      <div>
        <h2 class="text-xl font-semibold text-white">医学知识库与书籍导入工坊</h2>
        <p class="text-sm text-slate-400">
          支持多文件批量上传、自由文本粘贴，通过 AI 提示词与标准 Schema 统一规范化后再校验写入数据库
        </p>
      </div>
      <div class="flex rounded-lg bg-slate-900 p-1 border border-slate-800 text-xs">
        <button
          @click="activeMode = 'batch_ai'"
          :class="activeMode === 'batch_ai' ? 'bg-sky-600 text-white' : 'text-slate-400 hover:text-white'"
          class="px-3 py-1.5 rounded-md font-medium transition-colors"
        >
          AI 批量导入与规范化
        </button>
        <button
          @click="activeMode = 'local_path'"
          :class="activeMode === 'local_path' ? 'bg-sky-600 text-white' : 'text-slate-400 hover:text-white'"
          class="px-3 py-1.5 rounded-md font-medium transition-colors"
        >
          本地路径 / 断点续传
        </button>
      </div>
    </header>

    <!-- Existing Knowledge Base Categories Overview -->
    <div class="bg-slate-900 p-4 rounded-xl border border-slate-800 space-y-3">
      <div class="flex flex-wrap items-center justify-between gap-2">
        <div class="flex items-center gap-2">
          <h3 class="text-sm font-semibold text-white">已有知识库分类总览</h3>
          <span class="px-2 py-0.5 rounded-full bg-sky-950/80 border border-sky-700/60 text-xs text-sky-300">
            共 {{ kbCategories?.total_documents ?? 0 }} 条 · {{ kbCategories?.departments?.length ?? 0 }} 个科室分类
          </span>
        </div>
        <div class="flex items-center gap-2 text-xs">
          <button
            @click="loadCategories"
            :disabled="isLoadingCategories"
            class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-slate-300"
          >
            {{ isLoadingCategories ? '刷新中...' : '刷新分类' }}
          </button>
          <button
            v-if="(kbCategories?.total_documents ?? 0) > 0"
            @click="quickExportKnowledge(selectedBrowseDept)"
            class="px-2.5 py-1 rounded bg-sky-700 hover:bg-sky-600 text-white"
          >
            {{ selectedBrowseDept ? `导出「${selectedBrowseDept}」` : '快捷导出全部知识库' }}
          </button>
        </div>
      </div>

      <div v-if="!kbCategories || kbCategories.total_documents === 0" class="text-xs text-slate-500">
        当前知识库暂无已入库文档。导入知识后，这里会自动汇总展示全部科室分类、疾病覆盖与知识类型统计。
      </div>

      <template v-else>
        <!-- Department category chips -->
        <div class="flex flex-wrap gap-2">
          <button
            v-for="cat in kbCategories.departments"
            :key="cat.department"
            @click="browseDepartment(cat.department)"
            :class="
              selectedBrowseDept === cat.department
                ? 'bg-sky-600 text-white border-sky-400'
                : 'bg-slate-950 text-slate-200 border-slate-700 hover:border-sky-600'
            "
            class="px-3 py-1.5 rounded-lg border text-xs transition-colors flex items-center gap-2"
            :title="`覆盖疾病：${cat.diseases.slice(0, 8).join('、')}`"
          >
            <span class="font-medium">{{ cat.department }}</span>
            <span class="px-1.5 py-0.5 rounded bg-slate-800/80 text-[11px] text-sky-300">{{ cat.doc_count }} 条</span>
            <span class="text-[11px] opacity-70">{{ cat.diseases.length }} 种疾病</span>
          </button>
        </div>

        <!-- Section types & tags distribution -->
        <div class="flex flex-wrap items-center gap-2 text-[11px] text-slate-400 pt-1">
          <span>知识类型分布：</span>
          <span
            v-for="st in kbCategories.section_types"
            :key="st.name"
            class="px-2 py-0.5 rounded bg-slate-950 border border-slate-800 text-slate-300"
          >
            {{ sectionTypeLabel[st.name] || st.name }} ({{ st.count }})
          </span>
          <template v-if="kbCategories.seasonal_tags.length > 0">
            <span class="ml-2">热门标签：</span>
            <span
              v-for="tag in kbCategories.seasonal_tags.slice(0, 8)"
              :key="tag.name"
              class="px-2 py-0.5 rounded bg-amber-950/60 border border-amber-800/50 text-amber-200"
            >
              #{{ tag.name }} ({{ tag.count }})
            </span>
          </template>
        </div>

        <!-- Browse documents in selected department -->
        <div v-if="selectedBrowseDept" class="pt-3 border-t border-slate-800 space-y-2">
          <div class="flex items-center justify-between text-xs">
            <span class="text-sky-300 font-medium">
              「{{ selectedBrowseDept }}」已有知识条目（显示 {{ browseDocs.length }} 条）
            </span>
            <button @click="selectedBrowseDept = null; browseDocs = []" class="text-slate-400 hover:text-white">
              收起
            </button>
          </div>
          <div class="grid grid-cols-1 md:grid-cols-2 gap-2 max-h-60 overflow-y-auto pr-1">
            <div
              v-for="doc in browseDocs"
              :key="doc.doc_id"
              class="p-2.5 rounded-lg bg-slate-950 border border-slate-800 text-xs space-y-1"
            >
              <div class="flex items-center justify-between gap-2">
                <span class="font-semibold text-white truncate">{{ doc.disease_name }}</span>
                <span class="px-1.5 py-0.5 rounded bg-slate-800 text-[10px] text-slate-300">
                  {{ sectionTypeLabel[doc.section_type] || doc.section_type }}
                </span>
              </div>
              <p class="text-[11px] text-slate-400 line-clamp-2">{{ doc.content }}</p>
            </div>
          </div>
        </div>
      </template>
    </div>

    <!-- Mode 1: Batch Upload + AI Normalization -->
    <div v-if="activeMode === 'batch_ai'" class="space-y-4">
      <div class="bg-slate-900 p-4 rounded-xl border border-slate-800 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 class="text-sm font-semibold text-white">1. 选择或拖入多格式资料（支持批量导入）</h3>
            <p class="text-xs text-slate-400">
              支持 TXT、Markdown、JSON/JSONL、CSV、Excel、DOCX、PDF、HTML、XML 等任意格式混合导入
            </p>
          </div>
          <div class="flex items-center gap-2">
            <input
              ref="fileInputRef"
              type="file"
              multiple
              accept=".txt,.md,.markdown,.json,.jsonl,.csv,.tsv,.xlsx,.xls,.docx,.pdf,.xml,.yaml,.yml,.html,.htm,.log"
              class="hidden"
              @change="handleFileInputChange"
            />
            <button
              @click="triggerFileSelect"
              class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium text-white"
            >
              + 导入文件（支持多选）
            </button>
            <button
              v-if="queuedFiles.length > 0"
              @click="clearQueue"
              class="px-3 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-xs text-slate-300"
            >
              清空队列
            </button>
          </div>
        </div>

        <!-- Drag & drop zone -->
        <div
          @dragover.prevent="isDragging = true"
          @dragleave.prevent="isDragging = false"
          @drop.prevent="handleDrop"
          @click="triggerFileSelect"
          :class="isDragging ? 'border-sky-400 bg-sky-950/30' : 'border-slate-700 hover:border-slate-500 bg-slate-950/60'"
          class="border-2 border-dashed rounded-xl p-5 text-center cursor-pointer transition-colors"
        >
          <p class="text-sm text-slate-200">点击此处或将多个医学文档拖拽到这里</p>
          <p class="text-xs text-slate-500 mt-1">支持一次选择多个不同格式的文件，AI 模型会自动统一转换为标准知识库结构</p>
        </div>

        <!-- Direct Clinical Image Attachment (Drag / Click / Ctrl+V) -->
        <div class="pt-2 border-t border-slate-800 space-y-2">
          <div class="flex items-center justify-between">
            <span class="text-xs font-medium text-sky-300">附带医学参考图片（伤口图、核磁/CT图谱、化验单、诱因物质图，入库时自动绑定到知识条目）</span>
            <span v-if="kbAttachedMedia.length" class="text-[11px] text-emerald-400">已附带 {{ kbAttachedMedia.length }} 张图片</span>
          </div>
          <ClinicalMediaDropzone v-model="kbAttachedMedia" compact />
        </div>

        <!-- Queued files list -->
        <div v-if="queuedFiles.length > 0" class="space-y-1.5">
          <div class="text-xs text-slate-400">待处理队列（{{ queuedFiles.length }} 项）：</div>
          <div class="flex flex-wrap gap-2">
            <div
              v-for="item in queuedFiles"
              :key="item.id"
              class="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs"
            >
              <span class="px-1.5 py-0.5 rounded bg-slate-800 text-sky-300">
                {{ item.kind === 'paste' ? '粘贴' : '文件' }}
              </span>
              <span class="text-slate-200 max-w-[220px] truncate">{{ item.name }}</span>
              <span class="text-slate-500">{{ formatBytes(item.size) }}</span>
              <button @click.stop="removeQueuedItem(item.id)" class="text-rose-400 hover:text-rose-300 ml-1">✕</button>
            </div>
          </div>
        </div>

        <!-- Direct paste box -->
        <div class="space-y-2 pt-2 border-t border-slate-800">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <label class="text-xs text-slate-400">或直接粘贴任意格式医学资料（指南摘录、问答、表格、零散笔记均可）</label>
            <div class="flex items-center gap-2">
              <input
                v-model="pastedLabel"
                placeholder="来源名称"
                class="px-2.5 py-1 rounded bg-slate-950 border border-slate-700 text-xs w-44"
              />
              <button
                @click="handleAddPastedToQueue"
                class="px-3 py-1 rounded bg-slate-800 hover:bg-slate-700 text-xs text-sky-300"
              >
                加入批量队列
              </button>
            </div>
          </div>
          <textarea
            v-model="pastedText"
            rows="4"
            placeholder="例如：直接粘贴一段疾病科普、临床指南、表格内容或 JSON/Markdown 片段..."
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-xs font-mono text-slate-200"
          />
        </div>
      </div>

      <!-- AI Normalization settings -->
      <div class="bg-slate-900 p-4 rounded-xl border border-slate-800 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h3 class="text-sm font-semibold text-white">2. AI 规范化模型与格式配置</h3>
          <div class="flex items-center gap-2">
            <button
              @click="loadInitialConfig"
              class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-xs text-slate-300"
            >
              刷新模型列表
            </button>
            <button
              @click="showPromptEditor = !showPromptEditor"
              class="px-2.5 py-1 rounded bg-slate-800 hover:bg-slate-700 text-xs text-sky-300"
            >
              {{ showPromptEditor ? '收起提示词与规范格式' : '查看/编辑 AI 规范化提示词' }}
            </button>
          </div>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-4 gap-3">
          <div class="md:col-span-2">
            <label class="block text-xs text-slate-400 mb-1">规范化 AI 模型（来自模块 6 网关配置）</label>
            <select
              v-model="selectedProviderId"
              class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm"
            >
              <option v-if="providers.length === 0" value="">暂无已配置模型（请先前往模块 6 配置）</option>
              <option v-for="p in providers" :key="p.id" :value="p.id">
                {{ p.name }} ({{ p.model }}){{ p.enabled ? '' : ' [已禁用]' }}
              </option>
            </select>
          </div>
          <div>
            <label class="block text-xs text-slate-400 mb-1">默认来源标题</label>
            <input v-model="sourceTitle" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
          </div>
          <div>
            <label class="block text-xs text-slate-400 mb-1">默认科室 / 命名空间</label>
            <div class="flex gap-2">
              <input v-model="defaultDepartment" class="w-1/2 px-2.5 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
              <input v-model="tenantNamespace" class="w-1/2 px-2.5 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
            </div>
          </div>
        </div>

        <div v-if="showPromptEditor" class="space-y-2 pt-2 border-t border-slate-800">
          <div class="flex items-center justify-between">
            <span class="text-xs text-slate-400">System Prompt（决定 AI 如何将多样数据规范化为标准 KnowledgeDocument JSON）</span>
            <button
              @click="customPrompt = defaultPrompt"
              class="text-xs text-amber-300 hover:underline"
            >
              恢复默认提示词
            </button>
          </div>
          <textarea
            v-model="customPrompt"
            rows="10"
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-xs font-mono text-slate-200 leading-relaxed"
          />
        </div>

        <!-- Primary Action Buttons -->
        <div class="flex flex-wrap gap-2 pt-1">
          <button
            :disabled="isBusy"
            @click="handleAiNormalizePreview"
            class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-sm font-medium text-white"
          >
            {{ isBusy ? '处理中...' : 'AI 批量规范化预览（需人工核对后入库）' }}
          </button>
          <button
            :disabled="isBusy || normalizedDocs.length === 0"
            @click="handleCommitNormalizedDocs"
            class="px-4 py-2 rounded-lg bg-teal-600 hover:bg-teal-500 disabled:opacity-40 text-sm font-medium text-white"
          >
            人工核对无误 · 写入数据库 ({{ normalizedDocs.length }})
          </button>
          <button
            :disabled="isBusy"
            @click="handleRuleBatchPreview"
            class="px-4 py-2 rounded-lg bg-slate-700 hover:bg-slate-600 disabled:opacity-50 text-sm text-slate-200"
          >
            规则批量解析（不调用 AI）
          </button>
        </div>
      </div>
    </div>

    <!-- Mode 2: Local Path & Resumable Mode (preserved) -->
    <div v-else class="grid grid-cols-1 md:grid-cols-3 gap-4 bg-slate-900 p-4 rounded-xl border border-slate-800">
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
        <button :disabled="isBusy" @click="handlePreview" class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-sm font-medium text-white">切片预览</button>
        <button :disabled="isBusy" @click="handleStartImport" class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-sm font-medium text-white">开始双写入库</button>
        <button :disabled="isBusy" @click="handleResumeImport" class="px-4 py-2 rounded-lg bg-amber-600 hover:bg-amber-500 disabled:opacity-50 text-sm font-medium text-white">断点续传</button>
        <button :disabled="isBusy" @click="handleRefreshStatus" class="px-4 py-2 rounded-lg bg-slate-700 hover:bg-slate-600 disabled:opacity-50 text-sm">查询检查点</button>
        <button :disabled="isBusy" @click="handleExportErrors" class="px-4 py-2 rounded-lg bg-rose-700 hover:bg-rose-600 disabled:opacity-50 text-sm text-white">导出错误日志</button>
      </div>
    </div>

    <!-- Status Banner -->
    <div
      v-if="statusMessage"
      :class="{
        'border-emerald-700 text-emerald-300': statusKind === 'success',
        'border-rose-700 text-rose-300': statusKind === 'error',
        'border-slate-700 text-sky-300': statusKind === 'info',
      }"
      class="p-3 rounded-lg bg-slate-900 border text-sm"
    >
      {{ statusMessage }}
    </div>

    <!-- Per-file batch summary -->
    <div v-if="itemResults.length > 0" class="p-4 rounded-xl bg-slate-900 border border-slate-800 space-y-2">
      <div class="flex items-center justify-between text-xs text-slate-400">
        <span>批量规范化明细（{{ itemResults.length }} 份资料）</span>
        <span v-if="lastAiMeta">模型：{{ lastAiMeta.model }} · 消耗 Tokens：{{ lastAiMeta.tokens }}</span>
      </div>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-2 text-xs">
        <div
          v-for="r in itemResults"
          :key="r.name"
          class="flex items-center justify-between px-3 py-2 rounded-lg bg-slate-950 border border-slate-800"
        >
          <span class="truncate text-slate-200">{{ r.name }}</span>
          <span :class="r.ok ? 'text-emerald-400' : 'text-rose-400'">
            {{ r.ok ? `已规范化 ${r.extracted_count} 条 (${r.segments_processed}段)` : (r.error || '失败') }}
          </span>
        </div>
      </div>
    </div>

    <!-- Errors / warnings -->
    <div v-if="normalizeErrors.length > 0" class="p-3 rounded-xl bg-rose-950/30 border border-rose-800/70 space-y-1 text-xs text-rose-300">
      <div class="font-semibold">解析/规范化告警（{{ normalizeErrors.length }}）：</div>
      <div v-for="(err, i) in normalizeErrors" :key="i" class="font-mono">{{ err }}</div>
    </div>

    <!-- Job Progress -->
    <div v-if="jobProgress" class="p-4 rounded-xl bg-slate-900 border border-slate-800 grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
      <div><span class="text-slate-400">状态：</span><span class="font-semibold text-emerald-400">{{ jobProgress.completed ? 'completed' : jobProgress.status }}</span></div>
      <div><span class="text-slate-400">已完成：</span>{{ jobProgress.processed_items }} / {{ jobProgress.total_items }}</div>
      <div><span class="text-slate-400">失败：</span>{{ jobProgress.failed_items }}</div>
      <div><span class="text-slate-400">偏移量：</span>{{ jobProgress.byte_offset }} B</div>
    </div>

    <!-- Normalized Knowledge Cards Preview -->
    <div v-if="normalizedDocs.length > 0" class="space-y-3">
      <div class="sticky top-0 z-10 flex flex-wrap items-center justify-between gap-2 py-2.5 px-3 rounded-xl bg-slate-900/95 backdrop-blur border border-slate-700/80 shadow-md">
        <h3 class="text-sm font-semibold text-white">标准格式预览（共 {{ normalizedDocs.length }} 条，建议先执行 AI 巡查分类再入库）</h3>
        <div class="flex flex-wrap items-center gap-2">
          <button
            :disabled="isBusy || isInspecting"
            @click="handleAiInspectKnowledge"
            class="px-3.5 py-1.5 rounded-lg bg-amber-600 hover:bg-amber-500 disabled:opacity-50 text-xs font-medium text-white"
          >
            {{ isInspecting ? 'AI 巡查分类中...' : 'AI 巡查分类' }}
          </button>
          <button
            v-if="inspectionResult"
            @click="applyAllAiClassifications"
            class="px-3 py-1.5 rounded-lg bg-sky-600 hover:bg-sky-500 text-xs font-medium text-white"
          >
            一键应用 AI 分类建议
          </button>
          <button
            v-if="inspectionResult && (inspectionResult.summary.reject_count > 0 || inspectionResult.summary.duplicate_count > 0)"
            @click="removeRejectedAndDuplicates"
            class="px-3 py-1.5 rounded-lg bg-rose-700 hover:bg-rose-600 text-xs font-medium text-white"
          >
            剔除异常与重复项
          </button>
          <button
            :disabled="isBusy || isInspecting"
            @click="handleCommitNormalizedDocs"
            class="px-4 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-xs font-medium text-white"
          >
            确认全部写入数据库 ({{ normalizedDocs.length }})
          </button>
        </div>
      </div>

      <!-- AI Inspection Summary Banner -->
      <div v-if="inspectionResult" class="p-3.5 rounded-xl bg-slate-900 border border-amber-700/60 space-y-2 text-xs">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div class="flex flex-wrap items-center gap-3">
            <span class="font-semibold text-amber-300">AI 巡查分类报告：</span>
            <span class="text-emerald-400">通过 {{ inspectionResult.summary.pass_count }}</span>
            <span class="text-amber-300">需复核 {{ inspectionResult.summary.warning_count }}</span>
            <span class="text-rose-400">建议剔除 {{ inspectionResult.summary.reject_count }}</span>
            <span class="text-sky-300">疑似重复 {{ inspectionResult.summary.duplicate_count }}</span>
          </div>
          <span class="text-slate-400">
            模型：{{ inspectionResult.model_used }} · Tokens：{{ inspectionResult.total_tokens }}
          </span>
        </div>
        <div v-if="inspectionResult.category_distribution?.length" class="flex flex-wrap items-center gap-1.5">
          <span class="text-slate-400">AI 归类科室分布：</span>
          <span
            v-for="c in inspectionResult.category_distribution"
            :key="c.name"
            class="px-2 py-0.5 rounded-full bg-indigo-950/80 border border-indigo-700/60 text-indigo-200"
          >
            {{ c.name }} ({{ c.count }})
          </span>
        </div>
      </div>

      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <article
          v-for="chunk in normalizedDocs"
          :key="chunk.doc_id"
          :class="
            inspectionByDocId[chunk.doc_id]?.status === 'reject'
              ? 'border-rose-700/80 bg-rose-950/20'
              : inspectionByDocId[chunk.doc_id]?.status === 'warning'
                ? 'border-amber-700/70 bg-amber-950/10'
                : 'border-slate-800 bg-slate-900'
          "
          class="p-4 rounded-xl border space-y-2"
        >
          <div class="flex items-center justify-between gap-2">
            <h3 class="font-semibold text-white">{{ chunk.disease_name }}</h3>
            <div class="flex items-center gap-2">
              <span
                v-if="inspectionByDocId[chunk.doc_id]"
                :class="
                  inspectionByDocId[chunk.doc_id].status === 'pass'
                    ? 'bg-emerald-950 text-emerald-300 border-emerald-700'
                    : inspectionByDocId[chunk.doc_id].status === 'warning'
                      ? 'bg-amber-950 text-amber-300 border-amber-700'
                      : 'bg-rose-950 text-rose-300 border-rose-700'
                "
                class="text-[11px] px-2 py-0.5 rounded-full border"
              >
                {{
                  inspectionByDocId[chunk.doc_id].status === 'pass'
                    ? '巡查通过'
                    : inspectionByDocId[chunk.doc_id].status === 'warning'
                      ? '需复核'
                      : '建议剔除'
                }}
              </span>
              <span class="text-xs px-2 py-0.5 rounded bg-sky-900 text-sky-200">{{ chunk.department }}</span>
              <button
                @click="removeNormalizedDoc(chunk.doc_id)"
                class="text-xs text-slate-500 hover:text-rose-400"
                title="移除此条不入库"
              >
                移除
              </button>
            </div>
          </div>

          <!-- Per-item AI Inspection Feedback -->
          <div
            v-if="inspectionByDocId[chunk.doc_id]"
            class="p-2.5 rounded-lg bg-slate-950/80 border border-slate-800 text-[11px] space-y-1"
          >
            <div class="flex flex-wrap items-center gap-2 text-slate-300">
              <span v-if="inspectionByDocId[chunk.doc_id].suggested_department !== chunk.department" class="text-amber-300">
                建议归类科室：{{ inspectionByDocId[chunk.doc_id].suggested_department }}
              </span>
              <span v-if="inspectionByDocId[chunk.doc_id].is_duplicate" class="px-1.5 py-0.5 rounded bg-amber-900/60 text-amber-200">
                疑似与已有条目重复 {{ inspectionByDocId[chunk.doc_id].duplicate_of ? `(${inspectionByDocId[chunk.doc_id].duplicate_of})` : '' }}
              </span>
              <span class="text-slate-400">{{ inspectionByDocId[chunk.doc_id].recommendation }}</span>
            </div>
            <div v-if="inspectionByDocId[chunk.doc_id].issues?.length" class="text-rose-300">
              问题：{{ inspectionByDocId[chunk.doc_id].issues.join('；') }}
            </div>
          </div>

          <div class="flex flex-wrap gap-1.5 text-xs">
            <span class="px-2 py-0.5 rounded bg-slate-800 text-slate-300">{{ sectionTypeLabel[chunk.section_type] || chunk.section_type }}</span>
            <span class="px-2 py-0.5 rounded bg-slate-800 text-emerald-300">权威度 {{ chunk.authority_level }}</span>
            <span class="px-2 py-0.5 rounded bg-slate-800 text-sky-300">适用人群 {{ (chunk.target_population || []).join('/') }}</span>
            <span v-for="sym in (chunk.key_symptoms || []).slice(0, 5)" :key="sym" class="px-2 py-0.5 rounded bg-indigo-900/60 text-indigo-200">{{ sym }}</span>
            <span v-for="tag in chunk.seasonal_tags" :key="tag" class="px-2 py-0.5 rounded bg-amber-900/60 text-amber-200">#{{ tag }}</span>
          </div>
          <p class="text-xs text-slate-300 leading-relaxed whitespace-pre-wrap">{{ chunk.content }}</p>
        </article>
      </div>
    </div>
  </section>
</template>
