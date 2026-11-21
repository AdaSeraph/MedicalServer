const api = require('./utils/api');

App({
  globalData: {
    apiBaseUrl: 'http://127.0.0.1:8080',
    token: '',
    patientId: '',
    consentLongTermMemory: false,
  },

  onLaunch() {
    const savedToken = wx.getStorageSync('auth_token') || '';
    const savedPatientId = wx.getStorageSync('patient_id') || '';
    const savedConsent = Boolean(wx.getStorageSync('consent_long_term_memory'));

    this.globalData.token = savedToken;
    this.globalData.patientId = savedPatientId;
    this.globalData.consentLongTermMemory = savedConsent;

    if (!savedToken || !savedPatientId) {
      this.ensureLogin();
    }
  },

  ensureLogin() {
    return new Promise((resolve, reject) => {
      wx.login({
        success: (loginRes) => {
          if (!loginRes.code) {
            reject(new Error('wx.login 未返回 code'));
            return;
          }
          api
            .loginWeChat(loginRes.code)
            .then((auth) => {
              this.globalData.token = auth.token;
              this.globalData.patientId = auth.patient_id;
              wx.setStorageSync('auth_token', auth.token);
              wx.setStorageSync('patient_id', auth.patient_id);
              resolve(auth);
            })
            .catch(reject);
        },
        fail: reject,
      });
    });
  },
});
