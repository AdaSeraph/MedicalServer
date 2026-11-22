<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invokeStudio } from '../api/tauri';
import { useToast } from '../composables/useToast';

const toast = useToast();

interface ClinicalTypeDescriptor {
  code: string;
  name_zh: string;
  name_en: string;
  description: string;
}

interface LabTemplate {
  panel_category: string;
  loinc_code: string;
  item_code: string;
  item_name: string;
  unit: string;
  reference_range: string;
}

interface ClinicalCatalog {
  version: string;
  standards: string[];
  vital_sign_fields: ClinicalTypeDescriptor[];
  lab_panel_categories: ClinicalTypeDescriptor[];
  common_lab_analytes: LabTemplate[];
  imaging_modalities: ClinicalTypeDescriptor[];
  wound_time_dimensions: ClinicalTypeDescriptor[];
  trigger_categories: ClinicalTypeDescriptor[];
  media_categories: ClinicalTypeDescriptor[];
}

interface UploadedMedia {
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

const catalog = ref<ClinicalCatalog | null>(null);
const targetMode = ref<'patient' | 'knowledge'>('patient');
const activeSection = ref<'vitals' | 'labs' | 'imaging' | 'wound' | 'trigger' | 'catalog'>('vitals');
const isSubmitting = ref(false);

// Target identifiers
const patientSerial = ref('PAT-DEMO-001');
const chiefComplaint = ref('复诊医学检查与影像/生化数据归档');
const recommendedDepartment = ref('全科医学科');
const knowledgeDocId = ref('seed_flu_internal_01');
const knowledgeDocs = ref<Array<{ doc_id: string; disease_name: string; department: string }>>([]);

// 1. Vital signs
const bpSys = ref<number | null>(128);
const bpDia = ref<number | null>(82);
const heartRate = ref<number | null>(76);
const temperature = ref<number | null>(36.6);
const spo2 = ref<number | null>(98);
const glucose = ref<number | null>(5.4);
const painScore = ref<number | null>(null);
const vitalNotes = ref('');

// 2. Blood / Biochemistry Lab Panel
const labCategory = ref('complete_blood_count');
const labPanelName = ref('血常规与炎症生化检验');
const labSpecimen = ref('静脉血');
const labItems = ref<
  Array<{
    loinc_code: string;
    item_code: string;
    item_name: string;
    value_numeric: number | null;
    value_text: string;
    unit: string;
    reference_range: string;
    abnormal_flag: string;
  }>
>([
  {
    loinc_code: '6690-2',
    item_code: 'WBC',
    item_name: '白细胞计数',
    value_numeric: 11.2,
    value_text: '',
    unit: '10^9/L',
    reference_range: '3.5-9.5',
    abnormal_flag: 'high',
  },
  {
    loinc_code: '1988-5',
    item_code: 'CRP',
    item_name: 'C反应蛋白',
    value_numeric: 24.5,
    value_text: '',
    unit: 'mg/L',
    reference_range: '<10.0',
    abnormal_flag: 'high',
  },
]);

// 3. Imaging (MRI / CT / X-ray / US / ECG)
const imagingModality = ref('mri');
const imagingExamName = ref('颅脑核磁共振(MRI)平扫+DWI');
const imagingBodyPart = ref('颅脑');
const imagingFindings = ref('');
const imagingImpression = ref('');
const imagingContrast = ref(false);
const imagingPhotos = ref<UploadedMedia[]>([]);

// 4. Wound assessment (TIME framework) + wound photos
const woundBodySite = ref('');
const woundType = ref('外伤裂伤');
const woundLength = ref<number | null>(null);
const woundWidth = ref<number | null>(null);
const woundDepth = ref<number | null>(null);
const woundTissue = ref('肉芽组织');
const woundInfectionText = ref('');
const woundExudate = ref('少量浆液性');
const woundEdge = ref('边缘整齐');
const woundNotes = ref('');
const woundPhotos = ref<UploadedMedia[]>([]);

// 5. Trigger substance / allergen + trigger photos
const triggerCategory = ref('drug');
const triggerName = ref('');
const triggerRoute = ref('口服/接触');
const triggerSymptomsText = ref('');
const triggerSeverity = ref('moderate');
const triggerNotes = ref('');
const triggerPhotos = ref<UploadedMedia[]>([]);

const lastResult = ref<string>('');

onMounted(async () => {
  try {
    catalog.value = await invokeStudio<ClinicalCatalog>('get_clinical_data_catalog');
  } catch (e) {
    toast.error(`加载医学检查数据类型目录失败: ${String(e)}`);
  }
  try {
    const docs = await invokeStudio<any[]>('list_knowledge_documents', {
      department: null,
      keyword: null,
      limit: 100,
    });
    knowledgeDocs.value = (docs ?? []).map((d) => ({
      doc_id: d.doc_id,
      disease_name: d.disease_name,
      department: d.department,
    }));
    if (knowledgeDocs.value.length > 0 && !knowledgeDocId.value) {
      knowledgeDocId.value = knowledgeDocs.value[0].doc_id;
    }
  } catch {
    // ignore if index is empty
  }
});

function addLabTemplate(tpl: LabTemplate) {
  labItems.value.push({
    loinc_code: tpl.loinc_code,
    item_code: tpl.item_code,
    item_name: tpl.item_name,
    value_numeric: null,
    value_text: '',
    unit: tpl.unit,
    reference_range: tpl.reference_range,
    abnormal_flag: 'normal',
  });
}

function addBlankLabItem() {
  labItems.value.push({
    loinc_code: '',
    item_code: '',
    item_name: '',
    value_numeric: null,
    value_text: '',
    unit: '',
    reference_range: '',
    abnormal_flag: 'normal',
  });
}

function removeLabItem(idx: number) {
  labItems.value.splice(idx, 1);
}

async function handleImageUpload(
  event: Event,
  targetList: UploadedMedia[],
  category: string,
  defaultTag: string,
) {
  const input = event.target as HTMLInputElement;
  if (!input.files || input.files.length === 0) return;
  for (const file of Array.from(input.files)) {
    const dataUrl = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result || ''));
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
    const base64 = dataUrl.includes(',') ? dataUrl.split(',')[1] : dataUrl;
    targetList.push({
      asset_id: `media_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`,
      category,
      title: file.name,
      mime_type: file.type || 'image/jpeg',
      data_base64: base64,
      tags: [defaultTag],
      previewUrl: dataUrl,
    });
  }
  input.value = '';
}

function stripPreview(list: UploadedMedia[]) {
  return list.map(({ previewUrl: _p, ...rest }) => rest);
}

async function submitClinicalData() {
  isSubmitting.value = true;
  lastResult.value = '';
  try {
    const validLabItems = labItems.value
      .filter((i) => i.item_name.trim().length > 0)
      .map((i) => ({
        loinc_code: i.loinc_code.trim() || null,
        item_code: i.item_code.trim() || null,
        item_name: i.item_name.trim(),
        value_numeric: i.value_numeric !== null && !Number.isNaN(i.value_numeric) ? Number(i.value_numeric) : null,
        value_text: i.value_text.trim() || null,
        unit: i.unit.trim() || null,
        reference_range: i.reference_range.trim() || null,
        abnormal_flag: i.abnormal_flag || null,
      }));

    const hasVitals =
      bpSys.value !== null ||
      bpDia.value !== null ||
      heartRate.value !== null ||
      temperature.value !== null ||
      spo2.value !== null ||
      glucose.value !== null;

    const vitalRecord = hasVitals
      ? {
          record_id: `vs_${Date.now()}`,
          systolic_bp_mmhg: bpSys.value,
          diastolic_bp_mmhg: bpDia.value,
          heart_rate_bpm: heartRate.value,
          temperature_c: temperature.value,
          spo2_percent: spo2.value,
          blood_glucose_mmol_l: glucose.value,
          pain_score_0_10: painScore.value,
          notes: vitalNotes.value.trim() || null,
        }
      : null;

    const triggerRecord = triggerName.value.trim()
      ? {
          trigger_id: `trig_${Date.now()}`,
          category: triggerCategory.value,
          substance_name: triggerName.value.trim(),
          exposure_route: triggerRoute.value.trim() || null,
          reaction_symptoms: triggerSymptomsText.value
            .split(/[,，、;；]+/)
            .map((s) => s.trim())
            .filter(Boolean),
          severity: triggerSeverity.value || null,
          notes: triggerNotes.value.trim() || null,
          images: stripPreview(triggerPhotos.value),
        }
      : null;

    if (targetMode.value === 'patient') {
      const clinicalExams = {
        vital_signs: vitalRecord ? [vitalRecord] : [],
        lab_panels:
          validLabItems.length > 0
            ? [
                {
                  panel_id: `lab_${Date.now()}`,
                  category: labCategory.value,
                  panel_name: labPanelName.value.trim() || '医学检验生化报告',
                  specimen: labSpecimen.value.trim() || null,
                  items: validLabItems,
                  summary: null,
                  attachments: [],
                },
              ]
            : [],
        imaging_exams:
          imagingFindings.value.trim() || imagingImpression.value.trim() || imagingPhotos.value.length > 0
            ? [
                {
                  exam_id: `img_${Date.now()}`,
                  modality: imagingModality.value,
                  exam_name: imagingExamName.value.trim() || '医学影像检查',
                  body_part: imagingBodyPart.value.trim(),
                  findings: imagingFindings.value.trim() || '附医学影像图片',
                  impression: imagingImpression.value.trim(),
                  contrast_used: imagingContrast.value,
                  images: stripPreview(imagingPhotos.value),
                },
              ]
            : [],
        wound_assessments:
          woundBodySite.value.trim() || woundPhotos.value.length > 0
            ? [
                {
                  wound_id: `wound_${Date.now()}`,
                  body_site: woundBodySite.value.trim() || '未标注部位',
                  wound_type: woundType.value.trim() || '临床创面',
                  length_cm: woundLength.value,
                  width_cm: woundWidth.value,
                  depth_cm: woundDepth.value,
                  tissue_type: woundTissue.value.trim() || null,
                  infection_signs: woundInfectionText.value
                    .split(/[,，、;；]+/)
                    .map((s) => s.trim())
                    .filter(Boolean),
                  exudate_level: woundExudate.value.trim() || null,
                  edge_status: woundEdge.value.trim() || null,
                  notes: woundNotes.value.trim() || null,
                  images: stripPreview(woundPhotos.value),
                },
              ]
            : [],
        trigger_substances: triggerRecord ? [triggerRecord] : [],
        media_assets: [
          ...stripPreview(imagingPhotos.value),
          ...stripPreview(woundPhotos.value),
          ...stripPreview(triggerPhotos.value),
        ],
      };

      const res = await invokeStudio<any>('attach_patient_clinical_exams', {
        req: {
          serial_or_id: patientSerial.value.trim(),
          chief_complaint: chiefComplaint.value.trim() || null,
          recommended_department: recommendedDepartment.value.trim() || null,
          baseline_vitals: vitalRecord,
          l2_trigger_substances: triggerRecord ? [triggerRecord] : [],
          clinical_exams: clinicalExams,
        },
      });
      lastResult.value = `已成功导入至患者加密病例 ${res.patient_id}（共 ${res.l3_visit_events?.length ?? 0} 条就诊/检查记录）`;
      toast.success(lastResult.value);
    } else {
      const refVitals: string[] = [];
      if (bpSys.value !== null && bpDia.value !== null) {
        refVitals.push(`血压参考 ${bpSys.value}/${bpDia.value} mmHg`);
      }
      if (temperature.value !== null) refVitals.push(`体温参考 ${temperature.value} ℃`);
      if (spo2.value !== null) refVitals.push(`血氧参考 ${spo2.value}%`);

      const res = await invokeStudio<any>('attach_knowledge_clinical_assets', {
        req: {
          doc_id: knowledgeDocId.value.trim(),
          tenant_namespace: 'default',
          reference_labs: validLabItems,
          reference_vitals: refVitals,
          imaging_features:
            imagingFindings.value.trim() || imagingPhotos.value.length > 0
              ? [
                  {
                    modality: imagingModality.value,
                    exam_name: imagingExamName.value.trim() || '影像学检查',
                    body_part: imagingBodyPart.value.trim(),
                    typical_findings: imagingFindings.value.trim() || '详见参考影像图',
                    diagnostic_significance: imagingImpression.value.trim() || null,
                    reference_images: stripPreview(imagingPhotos.value),
                  },
                ]
              : [],
          wound_features:
            woundBodySite.value.trim() || woundNotes.value.trim() || woundPhotos.value.length > 0
              ? [
                  {
                    wound_type: woundType.value.trim() || '典型创面',
                    common_sites: woundBodySite.value.trim() ? [woundBodySite.value.trim()] : [],
                    visual_characteristics: woundNotes.value.trim() || `${woundTissue.value} / ${woundExudate.value}`,
                    warning_signs: woundInfectionText.value
                      .split(/[,，、;；]+/)
                      .map((s) => s.trim())
                      .filter(Boolean),
                    reference_images: stripPreview(woundPhotos.value),
                  },
                ]
              : [],
          trigger_substances: triggerRecord ? [triggerRecord] : [],
          media_assets: [
            ...stripPreview(imagingPhotos.value),
            ...stripPreview(woundPhotos.value),
            ...stripPreview(triggerPhotos.value),
          ],
        },
      });
      lastResult.value = `已成功关联至知识库文档 ${res.doc_id}（${res.disease_name}），并已同步更新 Tantivy 全文检索索引`;
      toast.success(lastResult.value);
    }
  } catch (err) {
    toast.error(`导入失败: ${String(err)}`);
  } finally {
    isSubmitting.value = false;
  }
}
</script>

<template>
  <div class="space-y-4">
    <div class="rounded-xl border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 p-4">
      <div class="flex flex-wrap items-center justify-between gap-3 mb-4">
        <div>
          <h2 class="text-lg font-semibold text-slate-900 dark:text-white">
            多模态医学检查、生化数据与临床图像导入中心
          </h2>
          <p class="text-xs text-slate-500 dark:text-slate-400 mt-0.5">
            支持血压/生命体征、抽血生化(LOINC)、核磁MRI/CT/超声/心电、伤口TIME评估与照片、诱因物质与照片，一键写入患者病例或医学知识库
          </p>
        </div>
        <div class="flex items-center gap-2">
          <button
            class="px-3 py-1.5 rounded-lg text-xs font-medium border transition"
            :class="targetMode === 'patient'
              ? 'bg-sky-600 text-white border-sky-600'
              : 'bg-slate-50 dark:bg-slate-700 text-slate-700 dark:text-slate-200 border-slate-300 dark:border-slate-600'"
            @click="targetMode = 'patient'"
          >
            导入至患者病例 (L2/L3)
          </button>
          <button
            class="px-3 py-1.5 rounded-lg text-xs font-medium border transition"
            :class="targetMode === 'knowledge'
              ? 'bg-emerald-600 text-white border-emerald-600'
              : 'bg-slate-50 dark:bg-slate-700 text-slate-700 dark:text-slate-200 border-slate-300 dark:border-slate-600'"
            @click="targetMode = 'knowledge'"
          >
            关联至医学知识库
          </button>
        </div>
      </div>

      <!-- Target Selector -->
      <div v-if="targetMode === 'patient'" class="grid grid-cols-1 md:grid-cols-3 gap-3 mb-4">
        <div>
          <label class="block text-xs font-medium text-slate-600 dark:text-slate-300 mb-1">患者序列号 / 匿名 Patient ID</label>
          <input v-model="patientSerial" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-3 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs font-medium text-slate-600 dark:text-slate-300 mb-1">本次检查主诉 / 标题</label>
          <input v-model="chiefComplaint" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-3 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs font-medium text-slate-600 dark:text-slate-300 mb-1">归属科室</label>
          <input v-model="recommendedDepartment" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-3 py-1.5 text-sm" />
        </div>
      </div>
      <div v-else class="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
        <div>
          <label class="block text-xs font-medium text-slate-600 dark:text-slate-300 mb-1">选择或输入目标知识库文档 (doc_id)</label>
          <select v-if="knowledgeDocs.length > 0" v-model="knowledgeDocId" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-3 py-1.5 text-sm">
            <option v-for="d in knowledgeDocs" :key="d.doc_id" :value="d.doc_id">
              {{ d.doc_id }} — {{ d.disease_name }} ({{ d.department }})
            </option>
          </select>
          <input v-else v-model="knowledgeDocId" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-3 py-1.5 text-sm" />
        </div>
      </div>

      <!-- Sub-tabs -->
      <div class="flex flex-wrap gap-2 border-b border-slate-200 dark:border-slate-700 pb-2 mb-4">
        <button
          v-for="tab in [
            { id: 'vitals', label: '1. 血压与生命体征' },
            { id: 'labs', label: '2. 抽血与生化指标' },
            { id: 'imaging', label: '3. 核磁/CT/影像与图' },
            { id: 'wound', label: '4. 伤口评估与图片' },
            { id: 'trigger', label: '5. 诱因物质与图片' },
            { id: 'catalog', label: '标准医学数据类型目录' },
          ]"
          :key="tab.id"
          class="px-3 py-1.5 rounded-lg text-xs font-medium transition"
          :class="activeSection === tab.id
            ? 'bg-sky-100 dark:bg-sky-900/50 text-sky-700 dark:text-sky-300'
            : 'text-slate-600 dark:text-slate-400 hover:bg-slate-100 dark:hover:bg-slate-700'"
          @click="activeSection = tab.id as any"
        >
          {{ tab.label }}
        </button>
      </div>

      <!-- Section 1: Vitals -->
      <div v-if="activeSection === 'vitals'" class="grid grid-cols-2 md:grid-cols-4 gap-3">
        <div>
          <label class="block text-xs text-slate-500 mb-1">收缩压/高压 (mmHg)</label>
          <input v-model.number="bpSys" type="number" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">舒张压/低压 (mmHg)</label>
          <input v-model.number="bpDia" type="number" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">心率 (bpm)</label>
          <input v-model.number="heartRate" type="number" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">体温 (℃)</label>
          <input v-model.number="temperature" type="number" step="0.1" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">血氧 SpO2 (%)</label>
          <input v-model.number="spo2" type="number" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">血糖 (mmol/L)</label>
          <input v-model.number="glucose" type="number" step="0.1" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">疼痛评分 (0-10)</label>
          <input v-model.number="painScore" type="number" min="0" max="10" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
        <div>
          <label class="block text-xs text-slate-500 mb-1">备注</label>
          <input v-model="vitalNotes" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        </div>
      </div>

      <!-- Section 2: Labs -->
      <div v-if="activeSection === 'labs'" class="space-y-3">
        <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
          <div>
            <label class="block text-xs text-slate-500 mb-1">检验大类</label>
            <select v-model="labCategory" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm">
              <option v-for="c in catalog?.lab_panel_categories ?? []" :key="c.code" :value="c.code">
                {{ c.name_zh }}
              </option>
            </select>
          </div>
          <div>
            <label class="block text-xs text-slate-500 mb-1">报告单名称</label>
            <input v-model="labPanelName" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          </div>
          <div>
            <label class="block text-xs text-slate-500 mb-1">标本类型</label>
            <input v-model="labSpecimen" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          </div>
        </div>

        <div class="flex flex-wrap gap-1.5">
          <span class="text-xs text-slate-500 self-center mr-1">快捷添加 LOINC 生化项：</span>
          <button
            v-for="tpl in (catalog?.common_lab_analytes ?? []).slice(0, 12)"
            :key="tpl.loinc_code"
            class="px-2 py-0.5 rounded bg-slate-100 dark:bg-slate-700 text-xs hover:bg-sky-100 dark:hover:bg-sky-800"
            @click="addLabTemplate(tpl)"
          >
            + {{ tpl.item_code }} {{ tpl.item_name }}
          </button>
          <button class="px-2 py-0.5 rounded bg-sky-600 text-white text-xs" @click="addBlankLabItem">+ 自定义项</button>
        </div>

        <div class="space-y-2">
          <div v-for="(item, idx) in labItems" :key="idx" class="grid grid-cols-6 gap-2 items-center">
            <input v-model="item.item_name" placeholder="项目名称" class="rounded border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2 py-1 text-xs" />
            <input v-model="item.item_code" placeholder="简称(WBC/ALT)" class="rounded border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2 py-1 text-xs" />
            <input v-model.number="item.value_numeric" type="number" step="0.01" placeholder="数值" class="rounded border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2 py-1 text-xs" />
            <input v-model="item.unit" placeholder="单位" class="rounded border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2 py-1 text-xs" />
            <select v-model="item.abnormal_flag" class="rounded border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2 py-1 text-xs">
              <option value="normal">正常</option>
              <option value="high">偏高(H)</option>
              <option value="low">偏低(L)</option>
              <option value="critical_high">危急高值</option>
              <option value="positive">阳性(+)</option>
              <option value="negative">阴性(-)</option>
            </select>
            <button class="text-xs text-rose-600 hover:underline" @click="removeLabItem(idx)">删除</button>
          </div>
        </div>
      </div>

      <!-- Section 3: Imaging -->
      <div v-if="activeSection === 'imaging'" class="space-y-3">
        <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
          <div>
            <label class="block text-xs text-slate-500 mb-1">检查模态</label>
            <select v-model="imagingModality" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm">
              <option v-for="m in catalog?.imaging_modalities ?? []" :key="m.code" :value="m.code">{{ m.name_zh }}</option>
            </select>
          </div>
          <div>
            <label class="block text-xs text-slate-500 mb-1">检查名称</label>
            <input v-model="imagingExamName" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          </div>
          <div>
            <label class="block text-xs text-slate-500 mb-1">检查部位</label>
            <input v-model="imagingBodyPart" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          </div>
        </div>
        <textarea v-model="imagingFindings" rows="2" placeholder="影像所见（如：左侧基底节区可见片状T2高信号、DWI呈高信号）" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 p-2.5 text-sm"></textarea>
        <input v-model="imagingImpression" placeholder="影像印象/诊断提示" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        <div>
          <label class="block text-xs text-slate-500 mb-1">上传核磁/CT/超声/心电等影像图片</label>
          <input type="file" accept="image/*" multiple @change="(e) => handleImageUpload(e, imagingPhotos, imagingModality === 'mri' ? 'mri_image' : 'ct_image', '医学影像')" />
          <div v-if="imagingPhotos.length" class="flex flex-wrap gap-2 mt-2">
            <img v-for="p in imagingPhotos" :key="p.asset_id" :src="p.previewUrl" :alt="p.title" class="h-16 w-16 object-cover rounded border" />
          </div>
        </div>
      </div>

      <!-- Section 4: Wound -->
      <div v-if="activeSection === 'wound'" class="space-y-3">
        <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
          <input v-model="woundBodySite" placeholder="伤口部位（如：右足背）" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          <input v-model="woundType" placeholder="伤口类型（如：糖尿病足溃疡/裂伤）" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          <input v-model="woundTissue" placeholder="T-基底组织（肉芽/腐肉/焦痂）" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          <input v-model="woundExudate" placeholder="M-渗液量与性质" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          <input v-model="woundInfectionText" placeholder="I-感染征象（逗号分隔：红肿,脓液）" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm col-span-2" />
          <input v-model="woundEdge" placeholder="E-创缘状态" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm col-span-2" />
        </div>
        <textarea v-model="woundNotes" rows="2" placeholder="伤口详细评估记录" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 p-2.5 text-sm"></textarea>
        <div>
          <label class="block text-xs text-slate-500 mb-1">上传伤口/创面图片</label>
          <input type="file" accept="image/*" multiple @change="(e) => handleImageUpload(e, woundPhotos, 'wound_image', '伤口图片')" />
          <div v-if="woundPhotos.length" class="flex flex-wrap gap-2 mt-2">
            <img v-for="p in woundPhotos" :key="p.asset_id" :src="p.previewUrl" :alt="p.title" class="h-16 w-16 object-cover rounded border" />
          </div>
        </div>
      </div>

      <!-- Section 5: Trigger Substance -->
      <div v-if="activeSection === 'trigger'" class="space-y-3">
        <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
          <select v-model="triggerCategory" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm">
            <option v-for="t in catalog?.trigger_categories ?? []" :key="t.code" :value="t.code">{{ t.name_zh }}</option>
          </select>
          <input v-model="triggerName" placeholder="诱因物质/过敏原名称（如：隐翅虫/芒果/头孢）" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
          <select v-model="triggerSeverity" class="rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm">
            <option value="mild">轻度反应</option>
            <option value="moderate">中度反应</option>
            <option value="severe">重度反应</option>
            <option value="anaphylaxis">过敏性休克</option>
          </select>
        </div>
        <input v-model="triggerSymptomsText" placeholder="诱发临床症状（逗号分隔：荨麻疹,红斑,呼吸困难）" class="w-full rounded-lg border border-slate-300 dark:border-slate-600 bg-white dark:bg-slate-900 px-2.5 py-1.5 text-sm" />
        <div>
          <label class="block text-xs text-slate-500 mb-1">上传诱因物质/过敏原实物图片（药盒、食物、植物、昆虫、化学品等）</label>
          <input type="file" accept="image/*" multiple @change="(e) => handleImageUpload(e, triggerPhotos, 'trigger_substance_image', '诱因物质')" />
          <div v-if="triggerPhotos.length" class="flex flex-wrap gap-2 mt-2">
            <img v-for="p in triggerPhotos" :key="p.asset_id" :src="p.previewUrl" :alt="p.title" class="h-16 w-16 object-cover rounded border" />
          </div>
        </div>
      </div>

      <!-- Section 6: Catalog -->
      <div v-if="activeSection === 'catalog' && catalog" class="space-y-3 text-xs">
        <div class="p-3 rounded-lg bg-slate-50 dark:bg-slate-900/60">
          <div class="font-semibold mb-1">遵循国际临床数据标准 (v{{ catalog.version }})：</div>
          <ul class="list-disc pl-4 space-y-0.5 text-slate-600 dark:text-slate-300">
            <li v-for="s in catalog.standards" :key="s">{{ s }}</li>
          </ul>
        </div>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
          <div class="p-3 rounded-lg border border-slate-200 dark:border-slate-700">
            <div class="font-semibold mb-1">支持医学图像类别 ({{ catalog.media_categories.length }})</div>
            <div class="flex flex-wrap gap-1">
              <span v-for="m in catalog.media_categories" :key="m.code" class="px-2 py-0.5 rounded bg-sky-50 dark:bg-sky-900/40 text-sky-700 dark:text-sky-300">
                {{ m.name_zh }}
              </span>
            </div>
          </div>
          <div class="p-3 rounded-lg border border-slate-200 dark:border-slate-700">
            <div class="font-semibold mb-1">支持生化与检验大类 ({{ catalog.lab_panel_categories.length }})</div>
            <div class="flex flex-wrap gap-1">
              <span v-for="c in catalog.lab_panel_categories" :key="c.code" class="px-2 py-0.5 rounded bg-emerald-50 dark:bg-emerald-900/40 text-emerald-700 dark:text-emerald-300">
                {{ c.name_zh }}
              </span>
            </div>
          </div>
        </div>
      </div>

      <!-- Submit bar -->
      <div class="mt-4 pt-3 border-t border-slate-200 dark:border-slate-700 flex items-center justify-between">
        <div class="text-xs text-emerald-600 dark:text-emerald-400">{{ lastResult }}</div>
        <button
          :disabled="isSubmitting"
          class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-700 text-white text-sm font-medium disabled:opacity-50"
          @click="submitClinicalData"
        >
          {{ isSubmitting ? '正在写入...' : targetMode === 'patient' ? '写入患者加密病例' : '关联并索引至知识库' }}
        </button>
      </div>
    </div>
  </div>
</template>
