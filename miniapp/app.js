const api = require('./utils/api');
const { getApiBaseUrl } = require('./utils/config');

App({
  globalData: {
    apiBaseUrl: getApiBaseUrl(),
    token: '',
    tokenExpiresAt: 0,
    patientId: '',
    consentLongTermMemory: false,
  },

  _loginPromise: null,

  onLaunch() {
    const savedToken = wx.getStorageSync('auth_token') || '';
    const savedExpiresAt = Number(wx.getStorageSync('auth_token_expires_at') || 0);
    const savedPatientId = wx.getStorageSync('patient_id') || '';
    const savedConsent = this.getPatientConsent(savedPatientId);

    this.globalData.token = savedToken;
    this.globalData.tokenExpiresAt = savedExpiresAt;
    this.globalData.patientId = savedPatientId;
    this.globalData.consentLongTermMemory = savedConsent;

    if (!this.hasValidToken()) {
      this.ensureLogin(true).catch(() => {});
    }
  },

  hasValidToken() {
    const now = Date.now();
    // Refresh 60 seconds before actual JWT expiration to avoid mid-request 401s
    return Boolean(
      this.globalData.token &&
        this.globalData.patientId &&
        (!this.globalData.tokenExpiresAt || now < this.globalData.tokenExpiresAt - 60000)
    );
  },

  clearAuth() {
    this.globalData.token = '';
    this.globalData.tokenExpiresAt = 0;
    wx.removeStorageSync('auth_token');
    wx.removeStorageSync('auth_token_expires_at');
  },

  getPatientConsent(patientId) {
    const pid = patientId || this.globalData.patientId || wx.getStorageSync('patient_id') || '';
    if (pid) {
      const scoped = wx.getStorageSync(`consent_long_term_memory_${pid}`);
      if (scoped !== '' && scoped !== undefined && scoped !== null) {
        return Boolean(scoped);
      }
    }
    return Boolean(wx.getStorageSync('consent_long_term_memory'));
  },

  setPatientConsent(enabled, patientId) {
    const val = Boolean(enabled);
    const pid = patientId || this.globalData.patientId || wx.getStorageSync('patient_id') || '';
    this.globalData.consentLongTermMemory = val;
    wx.setStorageSync('consent_long_term_memory', val);
    if (pid) {
      wx.setStorageSync(`consent_long_term_memory_${pid}`, val);
    }
  },

  ensureLogin(forceRefresh) {
    if (!forceRefresh && this.hasValidToken()) {
      return Promise.resolve({
        token: this.globalData.token,
        patient_id: this.globalData.patientId,
      });
    }
    if (this._loginPromise) {
      return this._loginPromise;
    }

    this._loginPromise = new Promise((resolve, reject) => {
      wx.login({
        success: (loginRes) => {
          if (!loginRes.code) {
            reject(new Error('wx.login 未返回 code'));
            return;
          }
          api
            .loginWeChat(loginRes.code)
            .then((auth) => {
              const ttlSecs = Number(auth.expires_in || 7200);
              const expiresAt = Date.now() + ttlSecs * 1000;
              this.globalData.token = auth.token;
              this.globalData.tokenExpiresAt = expiresAt;
              this.globalData.patientId = auth.patient_id;
              wx.setStorageSync('auth_token', auth.token);
              wx.setStorageSync('auth_token_expires_at', expiresAt);
              wx.setStorageSync('patient_id', auth.patient_id);
              resolve(auth);
            })
            .catch(reject);
        },
        fail: reject,
      });
    }).finally(() => {
      this._loginPromise = null;
    });

    return this._loginPromise;
  },
});
