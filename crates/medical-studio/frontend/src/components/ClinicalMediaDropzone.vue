<script setup lang="ts">
import { ref } from 'vue';

export interface LocalClinicalMediaItem {
  asset_id: string;
  category: string;
  title: string;
  mime_type: string;
  data_base64: string;
  body_site?: string;
  caption?: string;
  tags: string[];
  previewUrl: string;
}

const props = withDefaults(
  defineProps<{
    modelValue: LocalClinicalMediaItem[];
    compact?: boolean;
    placeholder?: string;
  }>(),
  {
    compact: false,
    placeholder: '拖拽图片到此处、点击选择，或按 Ctrl+V 直接粘贴截图（伤口、核磁/CT、化验单、诱因物质）',
  },
);

const emit = defineEmits<{
  (e: 'update:modelValue', value: LocalClinicalMediaItem[]): void;
}>();

const fileInputRef = ref<HTMLInputElement | null>(null);
const isDragging = ref(false);
const defaultCategory = ref('wound_image');
const lightboxUrl = ref<string | null>(null);

const categoryOptions = [
  { value: 'wound_image', label: '伤口图片', badge: 'bg-rose-950/70 text-rose-300 border-rose-700/60' },
  { value: 'mri_image', label: '核磁 MRI', badge: 'bg-indigo-950/70 text-indigo-300 border-indigo-700/60' },
  { value: 'ct_image', label: 'CT / X线', badge: 'bg-sky-950/70 text-sky-300 border-sky-700/60' },
  { value: 'lab_report_image', label: '抽血化验单', badge: 'bg-emerald-950/70 text-emerald-300 border-emerald-700/60' },
  { value: 'trigger_substance_image', label: '诱因物质', badge: 'bg-amber-950/70 text-amber-300 border-amber-700/60' },
  { value: 'dermatology_image', label: '皮损/其他', badge: 'bg-purple-950/70 text-purple-300 border-purple-700/60' },
];

function categoryLabel(val: string): string {
  return categoryOptions.find((c) => c.value === val)?.label ?? '医学图片';
}

async function ingestFiles(files: FileList | File[]) {
  const next = [...props.modelValue];
  for (const file of Array.from(files)) {
    if (!file.type.startsWith('image/')) continue;
    const dataUrl = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || ''));
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
    const base64 = dataUrl.includes(',') ? dataUrl.split(',')[1] : dataUrl;
    const title = file.name && file.name !== 'image.png'
      ? file.name
      : `${categoryLabel(defaultCategory.value)}_${new Date().toTimeString().slice(0, 8).replace(/:/g, '')}.png`;
    next.push({
      asset_id: `media_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      category: defaultCategory.value,
      title,
      mime_type: file.type || 'image/png',
      data_base64: base64,
      body_site: '',
      caption: '',
      tags: [categoryLabel(defaultCategory.value)],
      previewUrl: dataUrl,
    });
  }
  emit('update:modelValue', next);
}

async function onFileChange(e: Event) {
  const input = e.target as HTMLInputElement;
  if (input.files && input.files.length > 0) {
    await ingestFiles(input.files);
    input.value = '';
  }
}

async function onDrop(e: DragEvent) {
  isDragging.value = false;
  if (e.dataTransfer?.files && e.dataTransfer.files.length > 0) {
    await ingestFiles(e.dataTransfer.files);
  }
}

async function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  if (!items) return;
  const files: File[] = [];
  for (const item of Array.from(items)) {
    if (item.type.startsWith('image/')) {
      const f = item.getAsFile();
      if (f) files.push(f);
    }
  }
  if (files.length > 0) {
    e.preventDefault();
    await ingestFiles(files);
  }
}

function updateItem(idx: number, patch: Partial<LocalClinicalMediaItem>) {
  const next = props.modelValue.map((item, i) => (i === idx ? { ...item, ...patch } : item));
  emit('update:modelValue', next);
}

function removeItem(idx: number) {
  const next = props.modelValue.filter((_, i) => i !== idx);
  emit('update:modelValue', next);
}
</script>

<template>
  <div class="space-y-2" @paste="onPaste">
    <!-- Quick Category Pills -->
    <div class="flex flex-wrap items-center gap-1.5 text-[11px]">
      <span class="text-slate-400">图片类型：</span>
      <button
        v-for="cat in categoryOptions"
        :key="cat.value"
        type="button"
        @click="defaultCategory = cat.value"
        :class="defaultCategory === cat.value
          ? 'bg-sky-600 text-white border-sky-500'
          : 'bg-slate-900/70 text-slate-300 border-slate-700 hover:border-slate-500'"
        class="px-2 py-0.5 rounded-full border transition-colors"
      >
        {{ cat.label }}
      </button>
    </div>

    <!-- Drop / Click / Paste Zone -->
    <div
      tabindex="0"
      @click="fileInputRef?.click()"
      @dragover.prevent="isDragging = true"
      @dragleave.prevent="isDragging = false"
      @drop.prevent="onDrop"
      :class="[
        isDragging ? 'border-sky-400 bg-sky-950/40' : 'border-slate-700 bg-slate-950/60 hover:border-sky-600/70',
        compact ? 'px-3 py-2' : 'px-4 py-3',
      ]"
      class="rounded-lg border border-dashed cursor-pointer transition-colors outline-none focus:border-sky-500 flex items-center justify-between gap-3"
    >
      <div class="flex items-center gap-2 text-xs text-slate-300">
        <span class="text-base">📷</span>
        <span>{{ placeholder }}</span>
      </div>
      <span class="px-2.5 py-1 rounded bg-slate-800 text-sky-300 text-[11px] shrink-0">
        选择图片
      </span>
      <input
        ref="fileInputRef"
        type="file"
        accept="image/*"
        multiple
        class="hidden"
        @change="onFileChange"
      />
    </div>

    <!-- Thumbnail Cards -->
    <div v-if="modelValue.length > 0" class="grid grid-cols-1 sm:grid-cols-2 gap-2">
      <div
        v-for="(item, idx) in modelValue"
        :key="item.asset_id"
        class="flex items-start gap-2.5 p-2 rounded-lg bg-slate-950 border border-slate-800"
      >
        <img
          :src="item.previewUrl"
          :alt="item.title"
          @click.stop="lightboxUrl = item.previewUrl"
          title="点击查看大图"
          class="h-14 w-14 rounded object-cover border border-slate-700 cursor-zoom-in shrink-0"
        />
        <div class="flex-1 min-w-0 space-y-1">
          <div class="flex items-center justify-between gap-1">
            <select
              :value="item.category"
              @change="(e) => updateItem(idx, { category: (e.target as HTMLSelectElement).value })"
              class="px-1.5 py-0.5 rounded bg-slate-900 border border-slate-700 text-[11px] text-sky-300"
            >
              <option v-for="c in categoryOptions" :key="c.value" :value="c.value">{{ c.label }}</option>
            </select>
            <button
              type="button"
              @click="removeItem(idx)"
              class="text-[11px] text-rose-400 hover:text-rose-300"
            >
              移除
            </button>
          </div>
          <input
            :value="item.caption || ''"
            @input="(e) => updateItem(idx, { caption: (e.target as HTMLInputElement).value })"
            placeholder="补充描述（如：右足背伤口 / 颅脑MRI / 疑似致敏物）"
            class="w-full px-2 py-0.5 rounded bg-slate-900 border border-slate-800 text-[11px] text-slate-200"
          />
        </div>
      </div>
    </div>

    <!-- Fullscreen Lightbox -->
    <div
      v-if="lightboxUrl"
      @click="lightboxUrl = null"
      class="fixed inset-0 z-50 bg-black/80 flex items-center justify-center p-6 cursor-zoom-out"
    >
      <img :src="lightboxUrl" class="max-h-[85vh] max-w-[90vw] rounded-lg shadow-2xl border border-slate-700" />
    </div>
  </div>
</template>
