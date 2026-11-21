const api = require('../../utils/api');

const DISCLAIMER =
  '本建议由 AI 智能导诊结合医学知识库生成，仅供参考，不替代执业医师诊断。';

Page({
  data: {
    disclaimerText: DISCLAIMER,
    sessionId: `wx_sess_${Date.now()}`,
    regionCode: 'CN-110000',
    inputText: '',
    loading: false,
    messages: [
      {
        id: 1,
        role: 'assistant',
        content: '您好，我是智能医疗分诊助手。请描述您的主要不适症状，我将为您推荐合适科室。',
      },
    ],
    quickOptions: [],
    scrollAnchor: 'msg_1',
    showConsentModal: false,
    consentLongTermMemory: false,
    showEmergencyModal: false,
    emergencyAlertMessage: '',
  },

  onLoad() {
    const accepted = Boolean(wx.getStorageSync('disclaimer_accepted'));
    const consent = Boolean(wx.getStorageSync('consent_long_term_memory'));
    this.setData({
      showConsentModal: !accepted,
      consentLongTermMemory: consent,
    });
  },

  toggleConsentChoice() {
    this.setData({
      consentLongTermMemory: !this.data.consentLongTermMemory,
    });
  },

  onAcceptDisclaimer() {
    wx.setStorageSync('disclaimer_accepted', true);
    wx.setStorageSync('consent_long_term_memory', this.data.consentLongTermMemory);
    const app = getApp();
    if (app && app.globalData) {
      app.globalData.consentLongTermMemory = this.data.consentLongTermMemory;
    }
    this.setData({ showConsentModal: false });
  },

  onInputChange(e) {
    this.setData({ inputText: e.detail.value });
  },

  onTapQuickOption(e) {
    const option = e.currentTarget.dataset.option;
    if (option) {
      this.submitUtterance(option);
    }
  },

  onSendMessage() {
    const text = (this.data.inputText || '').trim();
    if (!text || this.data.loading) return;
    this.setData({ inputText: '' });
    this.submitUtterance(text);
  },

  async submitUtterance(utterance) {
    const app = getApp();
    if (app && (!app.globalData.token || !app.globalData.patientId)) {
      try {
        await app.ensureLogin();
      } catch (e) {
        wx.showToast({ title: '登录失败，请重试', icon: 'none' });
        return;
      }
    }

    const userMsgId = Date.now();
    const botMsgId = userMsgId + 1;
    const nextMessages = this.data.messages.concat([
      { id: userMsgId, role: 'user', content: utterance },
      { id: botMsgId, role: 'assistant', content: '正在结合医学知识库分析...' },
    ]);

    this.setData({
      messages: nextMessages,
      quickOptions: [],
      loading: true,
      scrollAnchor: `msg_${botMsgId}`,
    });

    const payload = {
      session_id: this.data.sessionId,
      patient_id: (app && app.globalData.patientId) || wx.getStorageSync('patient_id') || '',
      region_code: this.data.regionCode,
      utterance,
      consent_long_term_memory: this.data.consentLongTermMemory,
      stream: true,
    };

    const handleResult = (resp) => {
      const updated = this.data.messages.map((m) => {
        if (m.id !== botMsgId) return m;
        if (resp.red_flag === 'Emergency120' || resp.status === 'emergency') {
          return Object.assign({}, m, {
            content: '【急危重症预警】请立即拨打 120 急救电话或前往最近急诊科！',
          });
        }
        if (resp.status === 'follow_up') {
          return Object.assign({}, m, { content: resp.question || '请补充您的症状细节：' });
        }
        return Object.assign({}, m, {
          content: (resp.triage_result && resp.triage_result.summary) || '分诊评估已完成，请查看下方科室建议。',
          triage_result: resp.triage_result || null,
        });
      });

      const isEmergency = resp.red_flag === 'Emergency120' || resp.status === 'emergency';
      this.setData({
        messages: updated,
        quickOptions: Array.isArray(resp.quick_options) ? resp.quick_options : [],
        loading: false,
        showEmergencyModal: isEmergency,
        emergencyAlertMessage: isEmergency
          ? '检测到高危急危重症表现，可能危及生命，请勿等待线上咨询，立即拨打 120 急救电话！'
          : '',
        scrollAnchor: `msg_${botMsgId}`,
      });
    };

    api.streamTriageChat(
      payload,
      (deltaObj) => {
        if (!deltaObj || !deltaObj.delta) return;
        const updated = this.data.messages.map((m) =>
          m.id === botMsgId
            ? Object.assign({}, m, {
                content:
                  m.content === '正在结合医学知识库分析...'
                    ? deltaObj.delta
                    : m.content + deltaObj.delta,
              })
            : m
        );
        this.setData({ messages: updated });
      },
      handleResult,
      () => {
        // Fallback to standard JSON request if chunked streaming unavailable
        api
          .sendTriageChat(Object.assign({}, payload, { stream: false }))
          .then(handleResult)
          .catch(() => {
            this.setData({ loading: false });
            wx.showToast({ title: '网络繁忙，请稍后再试', icon: 'none' });
          });
      }
    );
  },

  onCall120() {
    wx.makePhoneCall({
      phoneNumber: '120',
    });
  },

  onCloseEmergencyModal() {
    this.setData({ showEmergencyModal: false });
  },

  onGuideRegistration(e) {
    const dept = e.currentTarget.dataset.dept || '全科医学科';
    wx.showModal({
      title: '挂号引导',
      content: `即将为您跳转至医院挂号平台预约「${dept}」。\n${DISCLAIMER}`,
      showCancel: false,
    });
  },

  goToMemoryPage() {
    wx.navigateTo({ url: '/pages/memory/index' });
  },
});
