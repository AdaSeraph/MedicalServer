const api = require('../../utils/api');

const DISCLAIMER =
  '本建议由 AI 智能导诊结合医学知识库生成，仅供参考，不替代执业医师诊断。';

const POPULATION_OPTIONS = [
  { key: 'adult', label: '成人' },
  { key: 'child', label: '儿童' },
  { key: 'pregnant', label: '孕产妇' },
  { key: 'elderly', label: '老年人' },
];

const REGION_OPTIONS = [
  { code: 'CN-110000', name: '北京市' },
  { code: 'CN-310000', name: '上海市' },
  { code: 'CN-440100', name: '广州市' },
  { code: 'CN-440300', name: '深圳市' },
  { code: 'CN-510100', name: '成都市' },
  { code: 'CN-330100', name: '杭州市' },
];

const URGENCY_LABELS = {
  1: '一级 · 立即急诊',
  2: '二级 · 尽快就诊',
  3: '三级 · 常规门诊',
  4: '四级 · 择期咨询',
};

function createWelcomeMessages() {
  return [
    {
      id: 1,
      role: 'assistant',
      content: '您好，我是智能医疗分诊助手。请选择就诊人群并描述您的主要不适症状，我将为您推荐合适科室。',
    },
  ];
}

function generateSessionId() {
  return `wx_sess_${Date.now()}_${Math.floor(Math.random() * 100000)}`;
}

Page({
  _msgSeq: 2,
  _lastWipedAt: 0,

  data: {
    disclaimerText: DISCLAIMER,
    sessionId: '',
    populationOptions: POPULATION_OPTIONS,
    selectedPopulation: 'adult',
    regionOptions: REGION_OPTIONS,
    regionIndex: 0,
    regionCode: 'CN-110000',
    epidemicBanner: '',
    inputText: '',
    loading: false,
    sessionCompleted: false,
    currentTurn: 0,
    maxTurns: 4,
    messages: createWelcomeMessages(),
    quickOptions: [],
    scrollAnchor: 'msg_1',
    showConsentModal: false,
    showPrivacyDetail: false,
    consentDeclined: false,
    consentLongTermMemory: false,
    showEmergencyModal: false,
    showEmergencyBanner: false,
    emergencyAlertMessage: '',
    showUrgentBanner: false,
  },

  onLoad() {
    const app = getApp();
    const pid = (app && app.globalData && app.globalData.patientId) || wx.getStorageSync('patient_id') || '';
    const acceptedKey = pid ? `disclaimer_accepted_${pid}` : 'disclaimer_accepted';
    const accepted = Boolean(wx.getStorageSync(acceptedKey) || wx.getStorageSync('disclaimer_accepted'));
    const consent =
      app && typeof app.getPatientConsent === 'function'
        ? app.getPatientConsent(pid)
        : Boolean(wx.getStorageSync('consent_long_term_memory'));
    this._lastWipedAt = Number(wx.getStorageSync('memory_wiped_at') || 0);

    this.setData({
      sessionId: generateSessionId(),
      showConsentModal: !accepted,
      consentLongTermMemory: consent,
    });
    this.loadLocalEpidemics(this.data.regionCode);
  },

  onShow() {
    // Always re-sync consent & wipe state when returning from the Health Profile page (MINI-03)
    const app = getApp();
    const pid = (app && app.globalData && app.globalData.patientId) || wx.getStorageSync('patient_id') || '';
    const latestConsent =
      app && typeof app.getPatientConsent === 'function'
        ? app.getPatientConsent(pid)
        : Boolean(wx.getStorageSync('consent_long_term_memory'));
    const wipedAt = Number(wx.getStorageSync('memory_wiped_at') || 0);

    if (wipedAt > this._lastWipedAt) {
      this._lastWipedAt = wipedAt;
      this.resetConversation(latestConsent);
    } else if (latestConsent !== this.data.consentLongTermMemory) {
      this.setData({ consentLongTermMemory: latestConsent });
    }
  },

  loadLocalEpidemics(regionCode) {
    api
      .getCurrentEpidemics(regionCode)
      .then((res) => {
        const list = (res && res.contexts) || [];
        const activeNames = list
          .filter((c) => c && c.active && Number(c.intensity || 0) > 0)
          .map((c) => c.disease_name)
          .slice(0, 2);
        this.setData({
          epidemicBanner: activeNames.length
            ? `近期本地高发提醒：${activeNames.join('、')}，如伴发热或咳嗽请留意防护`
            : '',
        });
      })
      .catch(() => {
        this.setData({ epidemicBanner: '' });
      });
  },

  onSelectPopulation(e) {
    const pop = e.currentTarget.dataset.pop;
    if (pop) {
      this.setData({ selectedPopulation: pop });
    }
  },

  onRegionChange(e) {
    const idx = Number(e.detail.value || 0);
    const region = REGION_OPTIONS[idx] || REGION_OPTIONS[0];
    this.setData({ regionIndex: idx, regionCode: region.code });
    this.loadLocalEpidemics(region.code);
  },

  toggleConsentChoice() {
    this.setData({
      consentLongTermMemory: !this.data.consentLongTermMemory,
    });
  },

  onOpenPrivacyDetail() {
    this.setData({ showPrivacyDetail: true });
  },

  onClosePrivacyDetail() {
    this.setData({ showPrivacyDetail: false });
  },

  onDeclineDisclaimer() {
    this.setData({
      showConsentModal: false,
      consentDeclined: true,
    });
  },

  onReopenConsent() {
    this.setData({
      showConsentModal: true,
      consentDeclined: false,
    });
  },

  onAcceptDisclaimer() {
    const app = getApp();
    const pid = (app && app.globalData && app.globalData.patientId) || wx.getStorageSync('patient_id') || '';
    wx.setStorageSync('disclaimer_accepted', true);
    if (pid) {
      wx.setStorageSync(`disclaimer_accepted_${pid}`, true);
    }
    if (app && typeof app.setPatientConsent === 'function') {
      app.setPatientConsent(this.data.consentLongTermMemory, pid);
    } else {
      wx.setStorageSync('consent_long_term_memory', this.data.consentLongTermMemory);
    }
    this.setData({ showConsentModal: false, consentDeclined: false });
  },

  resetConversation(consentOverride) {
    this._msgSeq += 2;
    const nextConsent =
      typeof consentOverride === 'boolean' ? consentOverride : this.data.consentLongTermMemory;
    this.setData({
      sessionId: generateSessionId(),
      messages: createWelcomeMessages(),
      quickOptions: [],
      loading: false,
      sessionCompleted: false,
      currentTurn: 0,
      showEmergencyModal: false,
      showEmergencyBanner: false,
      emergencyAlertMessage: '',
      showUrgentBanner: false,
      consentLongTermMemory: nextConsent,
      scrollAnchor: 'msg_1',
    });
  },

  onStartNewTriage() {
    this.resetConversation();
    wx.showToast({ title: '已开启新导诊', icon: 'none' });
  },

  onInputChange(e) {
    this.setData({ inputText: e.detail.value });
  },

  onTapQuickOption(e) {
    if (this.data.loading) return;
    const option = e.currentTarget.dataset.option;
    if (option) {
      this.submitUtterance(option);
    }
  },

  onSendMessage() {
    if (this.data.loading) return;
    const text = (this.data.inputText || '').trim();
    if (!text) return;
    this.setData({ inputText: '' });
    this.submitUtterance(text);
  },

  onRetryUtterance(e) {
    if (this.data.loading) return;
    const utterance = e.currentTarget.dataset.utterance;
    if (utterance) {
      this.submitUtterance(utterance);
    }
  },

  async submitUtterance(utterance) {
    // Immediate re-entrancy lock BEFORE any async await (MINI-08)
    if (this.data.loading) return;
    if (this.data.consentDeclined) {
      this.onReopenConsent();
      return;
    }
    this.setData({ loading: true });

    const app = getApp();
    if (app && typeof app.ensureLogin === 'function' && !app.hasValidToken()) {
      try {
        await app.ensureLogin(false);
      } catch (e) {
        this.setData({ loading: false });
        wx.showToast({ title: '登录失败，请检查网络后重试', icon: 'none' });
        return;
      }
    }

    const userMsgId = this._msgSeq++;
    const botMsgId = this._msgSeq++;
    const nextMessages = this.data.messages.concat([
      { id: userMsgId, role: 'user', content: utterance },
      { id: botMsgId, role: 'assistant', content: '正在结合医学知识库分析...' },
    ]);

    this.setData({
      messages: nextMessages,
      quickOptions: [],
      scrollAnchor: `msg_${botMsgId}`,
    });

    const payload = {
      session_id: this.data.sessionId,
      patient_id: (app && app.globalData.patientId) || wx.getStorageSync('patient_id') || '',
      region_code: this.data.regionCode,
      target_population: [this.data.selectedPopulation],
      utterance,
      consent_long_term_memory: this.data.consentLongTermMemory,
      stream: true,
    };

    const handleResult = (resp) => {
      const isEmergency = resp.red_flag === 'Emergency120' || resp.status === 'emergency';
      const isUrgent =
        !isEmergency &&
        (resp.red_flag === 'Urgent' ||
          (resp.triage_result && Number(resp.triage_result.urgency_level) === 1));
      const backendSummary =
        (resp.triage_result && resp.triage_result.summary) ||
        '检测到高危急危重症表现，请勿等待线上咨询，立即拨打 120 急救电话或前往最近急诊科！';

      const enrichedCard = resp.triage_result
        ? Object.assign({}, resp.triage_result, {
            urgency_label:
              URGENCY_LABELS[resp.triage_result.urgency_level] ||
              `紧急度 ${resp.triage_result.urgency_level} 级`,
          })
        : null;

      const updated = this.data.messages.map((m) => {
        if (m.id !== botMsgId) return m;
        if (isEmergency) {
          return Object.assign({}, m, {
            content: '【急危重症红色警示】请立即拨打 120 或前往最近医院急诊科！',
            triage_result: enrichedCard,
            citations: Array.isArray(resp.citations) ? resp.citations : [],
            is_fallback: Boolean(resp.is_fallback),
          });
        }
        if (resp.status === 'follow_up') {
          return Object.assign({}, m, {
            content: resp.question || '请补充您的症状细节：',
            turnInfo: `第 ${resp.turn || 1}/${resp.max_turns || 4} 轮追问`,
          });
        }
        // Avoid rendering duplicate summary in both bubble AND card (MINI-10)
        return Object.assign({}, m, {
          content: resp.is_fallback
            ? '当前智能推理通道繁忙，已为您切换本地医学规则快速导诊，请查看下方建议卡：'
            : '分诊评估已完成，请查看下方科室推荐卡：',
          triage_result: enrichedCard,
          citations: Array.isArray(resp.citations) ? resp.citations : [],
          is_fallback: Boolean(resp.is_fallback),
        });
      });

      this.setData({
        messages: updated,
        quickOptions: Array.isArray(resp.quick_options) ? resp.quick_options : [],
        loading: false,
        sessionCompleted: isEmergency || resp.status === 'completed',
        currentTurn: Number(resp.turn || this.data.currentTurn),
        maxTurns: Number(resp.max_turns || this.data.maxTurns),
        showEmergencyModal: isEmergency,
        showEmergencyBanner: isEmergency || this.data.showEmergencyBanner,
        emergencyAlertMessage: isEmergency ? backendSummary : this.data.emergencyAlertMessage,
        showUrgentBanner: isUrgent || this.data.showUrgentBanner,
        scrollAnchor: `msg_${botMsgId}`,
      });
    };

    const handleFailure = () => {
      const updated = this.data.messages.map((m) =>
        m.id === botMsgId
          ? Object.assign({}, m, {
              content: '网络繁忙或服务暂不可用，未能完成本次分析。',
              failedUtterance: utterance,
            })
          : m
      );
      this.setData({ messages: updated, loading: false });
      wx.showToast({ title: '网络繁忙，请点击重试', icon: 'none' });
    };

    api.streamTriageChat(
      payload,
      (deltaObj) => {
        if (!deltaObj || !deltaObj.delta) return;
        const text = String(deltaObj.delta);
        // Never render raw unparsed JSON syntax to patients (MINI-02)
        if (text.trim().startsWith('{') || text.includes('"action"')) return;
        const updated = this.data.messages.map((m) =>
          m.id === botMsgId
            ? Object.assign({}, m, {
                content:
                  m.content === '正在结合医学知识库分析...' ? text : m.content + text,
              })
            : m
        );
        this.setData({ messages: updated });
      },
      handleResult,
      () => {
        // Fallback to standard JSON request if chunked streaming fails
        api
          .sendTriageChat(Object.assign({}, payload, { stream: false }))
          .then(handleResult)
          .catch(handleFailure);
      }
    );
  },

  onCall120() {
    wx.makePhoneCall({
      phoneNumber: '120',
      fail: () => {
        wx.showModal({
          title: '请手动拨打 120',
          content: '当前设备无法直接唤起拨号，请立即使用手机或座机拨打急救电话：120！',
          showCancel: false,
          confirmText: '我知道了',
        });
      },
    });
  },

  onCloseEmergencyModal() {
    // Keep persistent top red 120 banner visible after closing the modal (MINI-06)
    this.setData({ showEmergencyModal: false, showEmergencyBanner: true });
  },

  onGuideRegistration(e) {
    const dept = e.currentTarget.dataset.dept || '全科医学科';
    wx.showModal({
      title: '就诊挂号指引',
      content: `建议您预约「${dept}」。如本院尚未接入小程序直连挂号，请前往医院官方公众号/挂号窗口或通过“微信服务-医疗健康”选择该科室。\n\n${DISCLAIMER}`,
      confirmText: '复制科室名',
      cancelText: '关闭',
      success: (res) => {
        if (res.confirm && typeof wx.setClipboardData === 'function') {
          wx.setClipboardData({
            data: dept,
            success: () => wx.showToast({ title: `已复制：${dept}`, icon: 'none' }),
          });
        }
      },
    });
  },

  goToMemoryPage() {
    wx.navigateTo({ url: '/pages/memory/index' });
  },
});
