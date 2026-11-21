const DEFAULT_BASE_URL = 'http://127.0.0.1:8080';

function getBaseUrl() {
  try {
    const app = getApp();
    return (app && app.globalData && app.globalData.apiBaseUrl) || DEFAULT_BASE_URL;
  } catch (e) {
    return DEFAULT_BASE_URL;
  }
}

function getAuthHeader() {
  const token = wx.getStorageSync('auth_token') || '';
  return token ? { Authorization: `Bearer ${token}` } : {};
}

function request(method, path, data) {
  return new Promise((resolve, reject) => {
    wx.request({
      url: `${getBaseUrl()}${path}`,
      method,
      data,
      header: Object.assign({ 'Content-Type': 'application/json' }, getAuthHeader()),
      success(res) {
        if (res.statusCode >= 200 && res.statusCode < 300) {
          resolve(res.data);
        } else {
          reject(new Error((res.data && res.data.error) || `HTTP ${res.statusCode}`));
        }
      },
      fail: reject,
    });
  });
}

function loginWeChat(code) {
  return request('POST', '/v1/auth/wechat', { code });
}

function sendTriageChat(payload) {
  return request('POST', '/v1/triage/chat', payload);
}

function streamTriageChat(payload, onDelta, onResult, onError) {
  const reqTask = wx.request({
    url: `${getBaseUrl()}/v1/triage/chat`,
    method: 'POST',
    enableChunked: true,
    data: Object.assign({}, payload, { stream: true }),
    header: Object.assign(
      { 'Content-Type': 'application/json', Accept: 'text/event-stream' },
      getAuthHeader()
    ),
    fail(err) {
      if (onError) onError(err);
    },
  });

  if (reqTask && typeof reqTask.onChunkReceived === 'function') {
    let buffer = '';
    reqTask.onChunkReceived((res) => {
      const text = decodeUtf8(res.data);
      buffer += text;
      const blocks = buffer.split('\n\n');
      buffer = blocks.pop() || '';
      blocks.forEach((block) => {
        const lines = block.split('\n');
        let eventType = 'message';
        let dataStr = '';
        lines.forEach((line) => {
          if (line.startsWith('event:')) eventType = line.slice(6).trim();
          if (line.startsWith('data:')) dataStr += line.slice(5).trim();
        });
        if (!dataStr) return;
        try {
          const parsed = JSON.parse(dataStr);
          if (eventType === 'delta' && onDelta) onDelta(parsed);
          if (eventType === 'result' && onResult) onResult(parsed);
          if (eventType === 'error' && onError) onError(new Error(parsed.error || 'stream error'));
        } catch (e) {
          // ignore partial chunk parse error
        }
      });
    });
  }
  return reqTask;
}

function decodeUtf8(arrayBuffer) {
  const bytes = new Uint8Array(arrayBuffer);
  let out = '';
  for (let i = 0; i < bytes.length; i += 1) {
    out += String.fromCharCode(bytes[i]);
  }
  try {
    return decodeURIComponent(escape(out));
  } catch (e) {
    return out;
  }
}

function getCurrentEpidemics(regionCode, season) {
  const qs = `?region_code=${encodeURIComponent(regionCode || 'CN-110000')}&season=${encodeURIComponent(season || 'winter')}`;
  return request('GET', `/v1/epidemic/current${qs}`);
}

function getPatientMemory(patientId) {
  return request('GET', `/v1/patients/${encodeURIComponent(patientId)}/memory`);
}

function updatePatientMemory(patientId, record) {
  return request('PUT', `/v1/patients/${encodeURIComponent(patientId)}/memory`, record);
}

function deletePatientMemory(patientId) {
  return request('DELETE', `/v1/patients/${encodeURIComponent(patientId)}/memory`);
}

module.exports = {
  loginWeChat,
  sendTriageChat,
  streamTriageChat,
  getCurrentEpidemics,
  getPatientMemory,
  updatePatientMemory,
  deletePatientMemory,
};
