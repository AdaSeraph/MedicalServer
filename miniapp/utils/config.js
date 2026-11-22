// Environment-aware API base URL configuration (avoids hardcoding plaintext HTTP in production).
const ENV_URLS = {
  develop: 'http://127.0.0.1:8080',
  trial: 'https://triage-staging.example.hospital.cn',
  release: 'https://triage.example.hospital.cn',
};

function getApiBaseUrl() {
  try {
    if (typeof wx !== 'undefined' && typeof wx.getAccountInfoSync === 'function') {
      const info = wx.getAccountInfoSync();
      const envVersion = info && info.miniProgram && info.miniProgram.envVersion;
      if (envVersion && ENV_URLS[envVersion]) {
        return ENV_URLS[envVersion];
      }
    }
  } catch (e) {
    // Fallback to develop endpoint in non-WeChat test environments
  }
  return ENV_URLS.develop;
}

module.exports = {
  ENV_URLS,
  getApiBaseUrl,
};
