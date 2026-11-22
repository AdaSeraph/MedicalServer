<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';
import {
  formatBytes,
  readBrowserFilesToQueue,
  type BatchImportItem,
  type QueuedImportFile,
} from '../utils/batchFiles';

const toast = useToast();

interface LlmProviderItem {
  id: string;
  name: string;
  model: string;
  enabled: boolean;
}

const activeMode = ref<'batch_ai' | 'local_path'>('batch_ai');

// Batch & AI normalization state
const fileInputRef = ref<HTMLInputElement | null>(null);
const isDragging = ref(false);
const queuedFiles = ref<QueuedImportFile[]>([]);
const pastedText = ref('');
const pastedLabel = ref('自由粘贴病例.txt');

const providers = ref<LlmProviderItem[]>([]);
const selectedProviderId = ref('');
const customPrompt = ref('');
const defaultPrompt = ref('');
const showPromptEditor = ref(false);
const preScrubPii = ref(true);

const salt = ref('medical-server-hospital-salt-v1');
const filePath = ref('e2e/fixtures/patients/raw_pii_patients.csv');

const isBusy = ref(false);
const normalizedRecords = ref<any[]>([]);
const detectedPiiBadges = ref<string[]>([]);
const itemResults = ref<any[]>([]);
const errors = ref<string[]>([]);
const lastAiMeta = ref<{ provider: string; model: string; tokens: number } | null>(null);

const commitResult = ref<any>(null);
const message = ref('');
const messageKind = ref<'info' | 'success' | 'error'>('info');

function setStatus(msg: string, kind: 'info' | 'success' | 'error' = 'info') {
  message.value = msg;
  messageKind.value = kind;
  if (kind !== 'info') {
    toast.push(msg, kind);
  }
}

async function copyPatientId(pid: string) {
  try {
    await navigator.clipboard.writeText(pid);
    toast.success('已复制匿名 Patient ID');
  } catch {
    toast.error('复制失败');
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
    defaultPrompt.value = prompts.patient_prompt;
    if (!customPrompt.value) {
      customPrompt.value = prompts.patient_prompt;
    }
  } catch (err) {
    setStatus(`加载 AI 模型或提示词失败：${String(err)}`, 'error');
  }
}

onMounted(() => {
  loadInitialConfig();
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
    setStatus(`已添加 ${added.length} 个病例文件到批量导入队列（共 ${queuedFiles.value.length} 项）`, 'info');
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
    setStatus(`已拖入 ${added.length} 个病例文件到批量导入队列（共 ${queuedFiles.value.length} 项）`, 'info');
  } catch (err) {
    setStatus(`读取拖入文件失败：${String(err)}`, 'error');
  }
}

function handleAddPastedToQueue() {
  const text = pastedText.value.trim();
  if (!text) {
    setStatus('请先粘贴需要规范化的病例内容', 'error');
    return;
  }
  const label = pastedLabel.value.trim() || `粘贴病例_${queuedFiles.value.length + 1}.txt`;
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
  normalizedRecords.value = [];
  detectedPiiBadges.value = [];
  itemResults.value = [];
  errors.value = [];
  lastAiMeta.value = null;
}

function collectBatchItems(): BatchImportItem[] {
  const items: BatchImportItem[] = queuedFiles.value.map((q) => q.item);
  const inlineText = pastedText.value.trim();
  if (inlineText) {
    items.push({
      name: pastedLabel.value.trim() || '直接粘贴病例文本.txt',
      content_text: inlineText,
    });
  }
  return items;
}

async function handleAiNormalizePreview(): Promise<boolean> {
  const items = collectBatchItems();
  if (items.length === 0) {
    setStatus('请先点击「导入病例文件」选择文件，或在下方粘贴病例文本', 'error');
    return false;
  }
  isBusy.value = true;
  setStatus(`正在对 ${items.length} 份病例执行${preScrubPii.value ? '本地预脱敏 + ' : ''}AI 规范化与隐私防火墙校验...`, 'info');
  try {
    const res = await invokeStudio<any>('ai_normalize_patient_batch', {
      req: {
        items,
        provider_id: selectedProviderId.value || null,
        custom_prompt: customPrompt.value.trim() || null,
        salt: salt.value,
        pre_scrub_pii: preScrubPii.value,
      },
    });
    normalizedRecords.value = res.records ?? [];
    detectedPiiBadges.value = res.detected_pii_types ?? [];
    itemResults.value = res.item_results ?? [];
    errors.value = res.errors ?? [];
    lastAiMeta.value = {
      provider: res.provider_used,
      model: res.model_used,
      tokens: res.total_tokens ?? 0,
    };
    if (normalizedRecords.value.length > 0) {
      setStatus(
        `AI 病例规范化完成：共生成 ${normalizedRecords.value.length} 条匿名合规病例（模型：${res.model_used}，Tokens：${res.total_tokens}），核对后点击「确认写入数据库」`,
        'success',
      );
      return true;
    } else {
      setStatus('AI 规范化未生成有效病例记录，请查看下方告警或检查模型配置', 'error');
      return false;
    }
  } catch (err) {
    setStatus(`AI 病例规范化失败：${String(err)}`, 'error');
    return false;
  } finally {
    isBusy.value = false;
  }
}

async function handleRuleBatchPreview() {
  const items = collectBatchItems();
  if (items.length === 0) {
    setStatus('请先添加至少一个 CSV/XLSX/JSON 病例文件', 'error');
    return;
  }
  isBusy.value = true;
  try {
    const res = await invokeStudio<any>('batch_preview_patient_items', {
      req: {
        items,
        salt: salt.value,
      },
    });
    normalizedRecords.value = res.records ?? [];
    detectedPiiBadges.value = res.detected_pii_columns ?? [];
    errors.value = res.failed_rows ?? [];
    itemResults.value = [];
    lastAiMeta.value = null;
    setStatus(
      `规则批量预检完成：识别 ${detectedPiiBadges.value.length} 类敏感列，共 ${normalizedRecords.value.length} 条合规记录`,
      'success',
    );
  } catch (err) {
    setStatus(`规则批量预检失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function handleCommitNormalizedRecords() {
  if (normalizedRecords.value.length === 0) {
    setStatus('当前没有可入库的规范化病例，请先执行预检或 AI 规范化', 'error');
    return;
  }
  isBusy.value = true;
  try {
    commitResult.value = await invokeStudio<any>('commit_patient_records', {
      req: {
        records: normalizedRecords.value,
        detected_pii_columns: detectedPiiBadges.value,
      },
    });
    setStatus(
      `批量入库完成：已写入 ${commitResult.value.imported_patients} 位病人 L2 画像与 ${commitResult.value.imported_l3_events} 条 L3 就诊事件`,
      'success',
    );
  } catch (err) {
    setStatus(`写入数据库失败：${String(err)}`, 'error');
  } finally {
    isBusy.value = false;
  }
}

async function handleOneClickAiImport() {
  const ok = await handleAiNormalizePreview();
  if (ok && normalizedRecords.value.length > 0) {
    await handleCommitNormalizedRecords();
  }
}

function removeNormalizedRecord(index: number) {
  normalizedRecords.value.splice(index, 1);
}

// Local path mode (preserved)
async function handlePreviewLocalPath() {
  try {
    const res = await invokeStudio<any>('preview_patient_import', {
      filePath: filePath.value,
      salt: salt.value,
    });
    normalizedRecords.value = res.records ?? [];
    detectedPiiBadges.value = res.detected_pii_columns ?? [];
    errors.value = res.failed_rows ?? [];
    setStatus(
      `脱敏预检完成：识别 ${detectedPiiBadges.value.length} 个敏感列，共 ${normalizedRecords.value.length} 条合规记录`,
      'success',
    );
  } catch (err) {
    setStatus(`预检失败：${String(err)}`, 'error');
  }
}

async function handleCommitLocalPath() {
  try {
    commitResult.value = await invokeStudio<any>('apply_masking_and_import_patients', {
      filePath: filePath.value,
      salt: salt.value,
    });
    setStatus(
      `入库完成：已写入 ${commitResult.value.imported_patients} 位病人 L2 画像与 ${commitResult.value.imported_l3_events} 条 L3 事件`,
      'success',
    );
  } catch (err) {
    setStatus(`入库失败：${String(err)}`, 'error');
  }
}
</script>

<template>
  <section class="space-y-6">
    <header class="flex flex-wrap items-center justify-between gap-4">
      <div>
        <h2 class="text-xl font-semibold text-white">病人历史数据与病例批量导入中心</h2>
        <p class="text-sm text-slate-400">
          支持多格式病例批量上传与粘贴，先经本地 PII 防火墙与 AI 规范化为标准 L2/L3 结构，再加盐哈希写入数据库
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
          本地路径导入
        </button>
      </div>
    </header>

    <!-- Mode 1: Batch Upload + AI Normalization -->
    <div v-if="activeMode === 'batch_ai'" class="space-y-4">
      <div class="bg-slate-900 p-4 rounded-xl border border-slate-800 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 class="text-sm font-semibold text-white">1. 选择或拖入多格式病例文件（支持批量导入）</h3>
            <p class="text-xs text-slate-400">
              支持门诊/住院病历文本、出院小结、CSV、Excel、JSON、DOCX、PDF、问诊记录等任意格式
            </p>
          </div>
          <div class="flex items-center gap-2">
            <input
              ref="fileInputRef"
              type="file"
              multiple
              accept=".csv,.xlsx,.xls,.json,.jsonl,.txt,.md,.markdown,.docx,.pdf,.xml,.html,.log"
              class="hidden"
              @change="handleFileInputChange"
            />
            <button
              @click="triggerFileSelect"
              class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium text-white"
            >
              + 导入病例文件（支持多选）
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
          <p class="text-sm text-slate-200">点击此处或将多个病例文件拖拽到这里</p>
          <p class="text-xs text-slate-500 mt-1">支持任意非结构化病历或异构表格，程序将在入库前强制执行 PII 脱敏与加盐 SHA-256 匿名化</p>
        </div>

        <!-- Queued files list -->
        <div v-if="queuedFiles.length > 0" class="space-y-1.5">
          <div class="text-xs text-slate-400">待处理病例队列（{{ queuedFiles.length }} 项）：</div>
          <div class="flex flex-wrap gap-2">
            <div
              v-for="item in queuedFiles"
              :key="item.id"
              class="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-slate-950 border border-slate-700 text-xs"
            >
              <span class="px-1.5 py-0.5 rounded bg-slate-800 text-emerald-300">
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
            <label class="text-xs text-slate-400">或直接粘贴任意格式病例/问诊记录（支持一次粘贴多份病例）</label>
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
            placeholder="例如：患者女，42岁，既往高血压5年，青霉素过敏。本次因反复头晕伴心悸2天就诊..."
            class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-xs font-mono text-slate-200"
          />
        </div>
      </div>

      <!-- AI Normalization & PII Firewall settings -->
      <div class="bg-slate-900 p-4 rounded-xl border border-slate-800 space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h3 class="text-sm font-semibold text-white">2. AI 病例规范化与隐私防火墙配置</h3>
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
              {{ showPromptEditor ? '收起提示词与规范格式' : '查看/编辑病例规范化提示词' }}
            </button>
          </div>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
          <div>
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
            <label class="block text-xs text-slate-400 mb-1">医院加盐密钥（Salt，用于不可逆匿名 ID）</label>
            <input v-model="salt" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
          </div>
          <div class="flex items-end pb-1">
            <label class="flex items-center gap-2 text-xs text-slate-200 cursor-pointer select-none">
              <input v-model="preScrubPii" type="checkbox" class="rounded border-slate-600 bg-slate-950" />
              发送给 AI 前本地预脱敏 PII（推荐开启，保护云端隐私）
            </label>
          </div>
        </div>

        <div v-if="showPromptEditor" class="space-y-2 pt-2 border-t border-slate-800">
          <div class="flex items-center justify-between">
            <span class="text-xs text-slate-400">System Prompt（决定 AI 如何将多样病例统一规范化为标准 L2/L3 JSON）</span>
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

        <!-- Action Buttons -->
        <div class="flex flex-wrap gap-2 pt-1">
          <button
            :disabled="isBusy"
            @click="handleAiNormalizePreview"
            class="px-4 py-2 rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-sm font-medium text-white"
          >
            {{ isBusy ? '处理中...' : 'AI 批量规范化 + 脱敏预检' }}
          </button>
          <button
            :disabled="isBusy"
            @click="handleOneClickAiImport"
            class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-sm font-medium text-white"
          >
            一键 AI 规范化并入库
          </button>
          <button
            :disabled="isBusy || normalizedRecords.length === 0"
            @click="handleCommitNormalizedRecords"
            class="px-4 py-2 rounded-lg bg-teal-600 hover:bg-teal-500 disabled:opacity-40 text-sm font-medium text-white"
          >
            确认无明文 PII 并批量写入数据库 ({{ normalizedRecords.length }})
          </button>
          <button
            :disabled="isBusy"
            @click="handleRuleBatchPreview"
            class="px-4 py-2 rounded-lg bg-slate-700 hover:bg-slate-600 disabled:opacity-50 text-sm text-slate-200"
          >
            规则批量预检（CSV/XLSX/JSON）
          </button>
        </div>
      </div>
    </div>

    <!-- Mode 2: Local Path (preserved) -->
    <div v-else class="grid grid-cols-1 md:grid-cols-3 gap-4 bg-slate-900 p-4 rounded-xl border border-slate-800">
      <div class="md:col-span-2">
        <label class="block text-xs text-slate-400 mb-1">病历文件路径（.csv / .xlsx / .json）</label>
        <input v-model="filePath" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
      </div>
      <div>
        <label class="block text-xs text-slate-400 mb-1">医院加盐密钥（Salt）</label>
        <input v-model="salt" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
      </div>
      <div class="md:col-span-3 flex gap-2">
        <button :disabled="isBusy" @click="handlePreviewLocalPath" class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-sm font-medium text-white">PII 脱敏防火墙预检</button>
        <button :disabled="isBusy" @click="handleCommitLocalPath" class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-sm font-medium text-white">确认无明文 PII 并批量入库</button>
      </div>
    </div>

    <!-- Status Banner -->
    <div
      v-if="message"
      :class="{
        'border-emerald-700 text-emerald-300': messageKind === 'success',
        'border-rose-700 text-rose-300': messageKind === 'error',
        'border-slate-700 text-sky-300': messageKind === 'info',
      }"
      class="p-3 rounded-lg bg-slate-900 border text-sm"
    >
      {{ message }}
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
    <div v-if="errors.length > 0" class="p-3 rounded-xl bg-rose-950/30 border border-rose-800/70 space-y-1 text-xs text-rose-300">
      <div class="font-semibold">解析/规范化告警（{{ errors.length }}）：</div>
      <div v-for="(err, i) in errors" :key="i" class="font-mono">{{ err }}</div>
    </div>

    <!-- Normalized & Anonymized Patient Records Table -->
    <div v-if="normalizedRecords.length > 0" class="space-y-4">
      <div v-if="detectedPiiBadges.length > 0" class="flex flex-wrap gap-2 items-center text-xs">
        <span class="text-slate-400">已自动拦截/脱敏敏感项：</span>
        <span
          v-for="col in detectedPiiBadges"
          :key="col"
          class="px-2 py-1 rounded bg-rose-900/70 text-rose-200 border border-rose-700"
        >
          {{ col }} (已脱敏/哈希)
        </span>
      </div>

      <div class="sticky top-0 z-10 flex items-center justify-between py-2.5 px-3 rounded-xl bg-slate-900/95 backdrop-blur border border-slate-700/80 shadow-md">
        <h3 class="text-sm font-semibold text-white">标准匿名病例预览（共 {{ normalizedRecords.length }} 位患者）</h3>
        <button
          :disabled="isBusy"
          @click="handleCommitNormalizedRecords"
          class="px-4 py-1.5 rounded-lg bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-xs font-medium text-white"
        >
          确认全部写入数据库 ({{ normalizedRecords.length }})
        </button>
      </div>

      <div class="overflow-x-auto rounded-xl border border-slate-800">
        <table class="w-full text-left text-xs">
          <thead class="bg-slate-900 text-slate-400">
            <tr>
              <th class="p-3">匿名 Patient ID</th>
              <th class="p-3">年龄段</th>
              <th class="p-3">性别</th>
              <th class="p-3">慢病史</th>
              <th class="p-3">过敏史</th>
              <th class="p-3">推荐科室 / 急诊级</th>
              <th class="p-3">脱敏后 L3 主诉与摘要</th>
              <th class="p-3">操作</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800 bg-slate-950">
            <tr v-for="(rec, idx) in normalizedRecords" :key="`${rec.patient_id}-${idx}`">
              <td class="p-3 font-mono text-sky-300">
                <button
                  @click="copyPatientId(rec.patient_id)"
                  :title="`点击复制完整 ID: ${rec.patient_id}`"
                  class="hover:underline text-left"
                >
                  {{ rec.patient_id.slice(0, 24) }}...
                </button>
              </td>
              <td class="p-3">{{ rec.l2_profile.age_group }}</td>
              <td class="p-3">{{ rec.l2_profile.gender }}</td>
              <td class="p-3">{{ (rec.l2_profile.chronic_diseases || []).join('、') || '无' }}</td>
              <td class="p-3 text-amber-300">{{ (rec.l2_profile.allergies || []).join('、') || '无' }}</td>
              <td class="p-3">
                <span v-if="rec.l3_visit_events?.length" class="px-2 py-0.5 rounded bg-sky-900/70 text-sky-200">
                  {{ rec.l3_visit_events[0].recommended_department }} · L{{ rec.l3_visit_events[0].urgency_level }}
                </span>
                <span v-else class="text-slate-500">-</span>
              </td>
              <td class="p-3 max-w-xs">
                <div class="text-slate-200">{{ rec.l3_visit_events?.[0]?.chief_complaint || '-' }}</div>
                <div v-if="rec.l3_visit_events?.[0]?.summary && rec.l3_visit_events[0].summary !== rec.l3_visit_events[0].chief_complaint" class="text-slate-400 mt-0.5">
                  {{ rec.l3_visit_events[0].summary }}
                </div>
              </td>
              <td class="p-3">
                <button @click="removeNormalizedRecord(idx)" class="text-rose-400 hover:text-rose-300">移除</button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </section>
</template>
