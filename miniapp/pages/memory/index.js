const api = require('../../utils/api');

const GENDER_OPTIONS = [
  { value: 'unknown', label: '保密/未设置' },
  { value: 'female', label: '女' },
  { value: 'male', label: '男' },
  { value: 'other', label: '其他' },
];

const AGE_GROUPS = ['0-17', '18-29', '30-39', '40-49', '50-59', '60-69', '70+'];

function formatUtc8(isoStr) {
  if (!isoStr) return '';
  const d = new Date(isoStr);
  if (Number.isNaN(d.getTime())) return isoStr;
  const utc8 = new Date(d.getTime() + 8 * 3600 * 1000);
  const pad = (n) => (n < 10 ? `0${n}` : `${n}`);
  return `${utc8.getUTCFullYear()}-${pad(utc8.getUTCMonth() + 1)}-${pad(utc8.getUTCDate())} ${pad(utc8.getUTCHours())}:${pad(utc8.getUTCMinutes())}`;
}

function maskPatientId(pid) {
  if (!pid) return '未生成';
  if (pid.length <= 16) return pid;
  return `${pid.slice(0, 12)}...${pid.slice(-6)}`;
}

function splitListText(text) {
  return String(text || '')
    .split(/[、,，;；\s]+/)
    .map((s) => s.trim())
    .filter((s) => s && s !== '无');
}

Page({
  data: {
    patientId: '',
    maskedPatientId: '未生成',
    consentLongTermMemory: false,
    loading: false,
    loadError: '',
    record: null,
    formattedEvents: [],
    editing: false,
    saving: false,
    genderOptions: GENDER_OPTIONS,
    genderIndex: 0,
    ageGroups: AGE_GROUPS,
    ageGroupIndex: 2,
    isPregnant: false,
    chronicText: '无',
    allergyText: '无',
    medicationText: '无',
    editChronic: '',
    editAllergy: '',
    editMedication: '',
  },

  onShow() {
    const app = getApp();
    const pid = (app && app.globalData && app.globalData.patientId) || wx.getStorageSync('patient_id') || '';
    const consent =
      app && typeof app.getPatientConsent === 'function'
        ? app.getPatientConsent(pid)
        : Boolean(wx.getStorageSync('consent_long_term_memory'));
    this.setData({
      patientId: pid,
      maskedPatientId: maskPatientId(pid),
      consentLongTermMemory: consent,
    });
    if (pid) {
      this.loadMemory(pid);
    }
  },

  onRetryLoad() {
    if (this.data.patientId) {
      this.loadMemory(this.data.patientId);
    }
  },

  loadMemory(patientId) {
    this.setData({ loading: true, loadError: '' });
    api
      .getPatientMemory(patientId)
      .then((rec) => {
        const l2 = (rec && rec.l2_profile) || {};
        const genderIdx = Math.max(
          0,
          GENDER_OPTIONS.findIndex((g) => g.value === (l2.gender || 'unknown'))
        );
        const ageIdx = Math.max(0, AGE_GROUPS.indexOf(l2.age_group || '30-39'));
        const events = ((rec && rec.l3_visit_events) || []).map((ev) =>
          Object.assign({}, ev, {
            formatted_time: formatUtc8(ev.occurred_at),
          })
        );
        const chronicStr = (l2.chronic_diseases || []).join('、') || '无';
        const allergyStr = (l2.allergies || []).join('、') || '无';
        const medStr = (l2.long_term_medications || []).join('、') || '无';

        const app = getApp();
        if (app && typeof app.setPatientConsent === 'function') {
          app.setPatientConsent(Boolean(rec.consent_long_term_memory), patientId);
        }

        this.setData({
          record: rec,
          formattedEvents: events,
          consentLongTermMemory: Boolean(rec.consent_long_term_memory),
          genderIndex: genderIdx,
          ageGroupIndex: ageIdx,
          isPregnant: Boolean(l2.is_pregnant),
          chronicText: chronicStr,
          allergyText: allergyStr,
          medicationText: medStr,
          editChronic: chronicStr === '无' ? '' : chronicStr,
          editAllergy: allergyStr === '无' ? '' : allergyStr,
          editMedication: medStr === '无' ? '' : medStr,
          loading: false,
          loadError: '',
        });
      })
      .catch((err) => {
        // Distinguish 404 (no archive yet) from real network/auth/500 errors (MINI-10)
        if (err && err.statusCode === 404) {
          this.setData({
            record: null,
            formattedEvents: [],
            loading: false,
            loadError: '',
          });
        } else {
          this.setData({
            loading: false,
            loadError: '加载健康档案失败，请检查网络或重新登录。',
          });
        }
      });
  },

  onToggleConsent(e) {
    const enabled = Boolean(e.detail.value);
    const prev = this.data.consentLongTermMemory;
    const app = getApp();
    const pid = this.data.patientId;

    this.setData({ consentLongTermMemory: enabled });
    if (!this.data.record || !pid) {
      if (app && typeof app.setPatientConsent === 'function') {
        app.setPatientConsent(enabled, pid);
      } else {
        wx.setStorageSync('consent_long_term_memory', enabled);
      }
      return;
    }

    // Send only L2 + consent flag (empty l3_visit_events avoids recomputing 50 L3 vectors on toggle)
    const updated = {
      patient_id: pid,
      consent_long_term_memory: enabled,
      l2_profile: this.data.record.l2_profile,
      l3_visit_events: [],
    };
    api
      .updatePatientMemory(pid, updated)
      .then(() => {
        if (app && typeof app.setPatientConsent === 'function') {
          app.setPatientConsent(enabled, pid);
        } else {
          wx.setStorageSync('consent_long_term_memory', enabled);
        }
        wx.showToast({ title: enabled ? '已开启长期记忆' : '已关闭长期记忆', icon: 'none' });
      })
      .catch(() => {
        // Rollback UI switch & storage on server PUT failure (MINI-03)
        this.setData({ consentLongTermMemory: prev });
        if (app && typeof app.setPatientConsent === 'function') {
          app.setPatientConsent(prev, pid);
        } else {
          wx.setStorageSync('consent_long_term_memory', prev);
        }
        wx.showToast({ title: '同步授权状态失败，请重试', icon: 'none' });
      });
  },

  onStartEdit() {
    this.setData({ editing: true });
  },

  onCancelEdit() {
    this.setData({ editing: false });
  },

  onAgeGroupChange(e) {
    this.setData({ ageGroupIndex: Number(e.detail.value || 0) });
  },

  onGenderChange(e) {
    this.setData({ genderIndex: Number(e.detail.value || 0) });
  },

  onPregnantChange(e) {
    this.setData({ isPregnant: Boolean(e.detail.value) });
  },

  onEditChronicInput(e) {
    this.setData({ editChronic: e.detail.value });
  },

  onEditAllergyInput(e) {
    this.setData({ editAllergy: e.detail.value });
  },

  onEditMedicationInput(e) {
    this.setData({ editMedication: e.detail.value });
  },

  onSaveProfile() {
    const pid = this.data.patientId;
    if (!pid || this.data.saving) return;
    this.setData({ saving: true });

    const payload = {
      patient_id: pid,
      consent_long_term_memory: this.data.consentLongTermMemory,
      l2_profile: {
        age_group: AGE_GROUPS[this.data.ageGroupIndex] || '30-39',
        gender: (GENDER_OPTIONS[this.data.genderIndex] || GENDER_OPTIONS[0]).value,
        is_pregnant: this.data.isPregnant,
        chronic_diseases: splitListText(this.data.editChronic),
        allergies: splitListText(this.data.editAllergy),
        surgical_history:
          (this.data.record &&
            this.data.record.l2_profile &&
            this.data.record.l2_profile.surgical_history) ||
          [],
        long_term_medications: splitListText(this.data.editMedication),
      },
      l3_visit_events: [],
    };

    api
      .updatePatientMemory(pid, payload)
      .then(() => {
        this.setData({ editing: false, saving: false });
        wx.showToast({ title: '健康档案已更新', icon: 'success' });
        this.loadMemory(pid);
      })
      .catch((err) => {
        this.setData({ saving: false });
        wx.showToast({ title: (err && err.message) || '保存失败，请检查内容', icon: 'none' });
      });
  },

  onWipeMemory() {
    const pid = this.data.patientId;
    if (!pid) return;
    wx.showModal({
      title: '确认一键清空健康档案？',
      content: '此操作将永久删除您的个人健康画像、历次分诊记录及当前对话缓存，无法撤销。',
      confirmColor: '#dc2626',
      success: (res) => {
        if (!res.confirm) return;
        api
          .deletePatientMemory(pid)
          .then(() => {
            const app = getApp();
            if (app && typeof app.setPatientConsent === 'function') {
              app.setPatientConsent(false, pid);
            } else {
              wx.setStorageSync('consent_long_term_memory', false);
            }
            // Signal triage page onShow() to clear local chat cache & sessionId (MINI-03)
            wx.setStorageSync('memory_wiped_at', Date.now());
            this.setData({
              record: null,
              formattedEvents: [],
              consentLongTermMemory: false,
              editing: false,
              chronicText: '无',
              allergyText: '无',
              medicationText: '无',
            });
            wx.showToast({ title: '已清空全部健康记录', icon: 'success' });
          })
          .catch(() => {
            wx.showToast({ title: '清空失败，请重试', icon: 'none' });
          });
      },
    });
  },
});
