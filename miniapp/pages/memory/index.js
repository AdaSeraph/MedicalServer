const api = require('../../utils/api');

Page({
  data: {
    patientId: '',
    consentLongTermMemory: false,
    loading: false,
    record: null,
    chronicText: '无',
    allergyText: '无',
    medicationText: '无',
  },

  onShow() {
    const pid = wx.getStorageSync('patient_id') || '';
    const consent = Boolean(wx.getStorageSync('consent_long_term_memory'));
    this.setData({ patientId: pid, consentLongTermMemory: consent });
    if (pid) {
      this.loadMemory(pid);
    }
  },

  loadMemory(patientId) {
    this.setData({ loading: true });
    api
      .getPatientMemory(patientId)
      .then((rec) => {
        const l2 = (rec && rec.l2_profile) || {};
        this.setData({
          record: rec,
          consentLongTermMemory: Boolean(rec.consent_long_term_memory),
          chronicText: (l2.chronic_diseases || []).join('、') || '无',
          allergyText: (l2.allergies || []).join('、') || '无',
          medicationText: (l2.long_term_medications || []).join('、') || '无',
          loading: false,
        });
      })
      .catch(() => {
        this.setData({ record: null, loading: false });
      });
  },

  onToggleConsent(e) {
    const enabled = Boolean(e.detail.value);
    wx.setStorageSync('consent_long_term_memory', enabled);
    this.setData({ consentLongTermMemory: enabled });
    if (this.data.record && this.data.patientId) {
      const updated = Object.assign({}, this.data.record, {
        consent_long_term_memory: enabled,
      });
      api.updatePatientMemory(this.data.patientId, updated).catch(() => {});
    }
  },

  onWipeMemory() {
    const pid = this.data.patientId;
    if (!pid) return;
    wx.showModal({
      title: '确认一键清空长期记忆？',
      content: '此操作将永久物理删除您的 L2 健康画像、L3 历史就诊记录与向量索引，无法撤销。',
      confirmColor: '#dc2626',
      success: (res) => {
        if (!res.confirm) return;
        api
          .deletePatientMemory(pid)
          .then(() => {
            wx.setStorageSync('consent_long_term_memory', false);
            this.setData({
              record: null,
              consentLongTermMemory: false,
              chronicText: '无',
              allergyText: '无',
              medicationText: '无',
            });
            wx.showToast({ title: '已物理清空全部记忆', icon: 'success' });
          })
          .catch(() => {
            wx.showToast({ title: '清空失败，请重试', icon: 'none' });
          });
      },
    });
  },
});
