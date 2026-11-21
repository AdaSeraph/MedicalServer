<script setup lang="ts">
import { ref } from 'vue';
import { invokeStudio } from '../api/tauri';

const filePath = ref('e2e/fixtures/patients/raw_pii_patients.csv');
const salt = ref('medical-server-hospital-salt-v1');
const previewReport = ref<any>(null);
const commitResult = ref<any>(null);
const message = ref('');

async function handlePreview() {
  try {
    previewReport.value = await invokeStudio<any>('preview_patient_import', {
      filePath: filePath.value,
      salt: salt.value,
    });
    message.value = `脱敏预检完成：识别 ${previewReport.value.detected_pii_columns.length} 个敏感列，共 ${previewReport.value.records.length} 条合规记录`;
  } catch (err) {
    message.value = `预检失败：${String(err)}`;
  }
}

async function handleCommit() {
  try {
    commitResult.value = await invokeStudio<any>('apply_masking_and_import_patients', {
      filePath: filePath.value,
      salt: salt.value,
    });
    message.value = `入库完成：已写入 ${commitResult.value.imported_patients} 位病人 L2 画像与 ${commitResult.value.imported_l3_events} 条 L3 事件`;
  } catch (err) {
    message.value = `入库失败：${String(err)}`;
  }
}
</script>

<template>
  <section class="space-y-6">
    <header>
      <h2 class="text-xl font-semibold text-white">病人历史数据批量导入与脱敏中心</h2>
      <p class="text-sm text-slate-400">自动检测姓名、身份证号、手机号、住址等 PII 列，入库前强制脱敏预览与加盐 SHA-256 匿名化</p>
    </header>

    <div class="grid grid-cols-1 md:grid-cols-3 gap-4 bg-slate-900 p-4 rounded-xl border border-slate-800">
      <div class="md:col-span-2">
        <label class="block text-xs text-slate-400 mb-1">病历文件路径（.csv / .xlsx / .json）</label>
        <input v-model="filePath" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
      </div>
      <div>
        <label class="block text-xs text-slate-400 mb-1">医院加盐密钥（Salt）</label>
        <input v-model="salt" class="w-full px-3 py-2 rounded-lg bg-slate-950 border border-slate-700 text-sm" />
      </div>
      <div class="md:col-span-3 flex gap-2">
        <button @click="handlePreview" class="px-4 py-2 rounded-lg bg-sky-600 hover:bg-sky-500 text-sm font-medium">PII 脱敏防火墙预检</button>
        <button @click="handleCommit" class="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-sm font-medium">确认无明文 PII 并批量入库</button>
      </div>
    </div>

    <div v-if="message" class="p-3 rounded-lg bg-slate-900 border border-slate-700 text-sm text-emerald-300">{{ message }}</div>

    <div v-if="previewReport" class="space-y-4">
      <div class="flex flex-wrap gap-2 items-center text-xs">
        <span class="text-slate-400">已自动拦截敏感列：</span>
        <span v-for="col in previewReport.detected_pii_columns" :key="col" class="px-2 py-1 rounded bg-rose-900/70 text-rose-200 border border-rose-700">
          {{ col }} (已脱敏/哈希)
        </span>
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
              <th class="p-3">脱敏后 L3 主诉</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800 bg-slate-950">
            <tr v-for="rec in previewReport.records" :key="rec.patient_id">
              <td class="p-3 font-mono text-sky-300">{{ rec.patient_id.slice(0, 26) }}...</td>
              <td class="p-3">{{ rec.l2_profile.age_group }}</td>
              <td class="p-3">{{ rec.l2_profile.gender }}</td>
              <td class="p-3">{{ rec.l2_profile.chronic_diseases.join('、') || '无' }}</td>
              <td class="p-3 text-amber-300">{{ rec.l2_profile.allergies.join('、') || '无' }}</td>
              <td class="p-3">{{ rec.l3_visit_events[0]?.chief_complaint || '-' }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </section>
</template>
