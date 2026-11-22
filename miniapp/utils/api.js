const { getApiBaseUrl } = require('./config');

function getBaseUrl() {
  try {
    const app = getApp();
    return (app && app.globalData && app.globalData.apiBaseUrl) || getApiBaseUrl();
  } catch (e) {
    return getApiBaseUrl();
  }
}

function getAuthHeader() {
  const token = wx.getStorageSync('auth_token') || '';
  return token ? { Authorization: `Bearer ${token}` } : {};
}

function tryRefreshAuth() {
  try {
    const app = getApp();
    if (app && typeof app.clearAuth === 'function' && typeof app.ensureLogin === 'function') {
      app.clearAuth();
      return app.ensureLogin(true);
    }
  } catch (e) {
    // ignore
  }
  return Promise.reject(new Error('无法刷新登录凭证'));
}

function request(method, path, data, allowRetry = true) {
  return new Promise((resolve, reject) => {
    wx.request({
      url: `${getBaseUrl()}${path}`,
      method,
      data,
      timeout: 20000,
      header: Object.assign({ 'Content-Type': 'application/json' }, getAuthHeader()),
      success(res) {
        if (res.statusCode >= 200 && res.statusCode < 300) {
          resolve(res.data);
        } else if (res.statusCode === 401 && allowRetry && path !== '/v1/auth/wechat') {
          tryRefreshAuth()
            .then(() => request(method, path, data, false))
            .then(resolve)
            .catch(reject);
        } else {
          const err = new Error((res.data && res.data.error) || `HTTP ${res.statusCode}`);
          err.statusCode = res.statusCode;
          reject(err);
        }
      },
      fail(err) {
        reject(err);
      },
    });
  });
}

function loginWeChat(code) {
  return request('POST', '/v1/auth/wechat', { code }, false);
}

function sendTriageChat(payload) {
  return request('POST', '/v1/triage/chat', payload);
}

/**
 * Stateful streaming UTF-8 decoder that buffers incomplete trailing multi-byte
 * sequences across TCP/chunk boundaries (prevents Chinese mojibake in WeChat enableChunked).
 */
function createUtf8StreamDecoder() {
  let pending = new Uint8Array(0);

  function decode(arrayBuffer) {
    const incoming = new Uint8Array(arrayBuffer);
    const bytes = new Uint8Array(pending.length + incoming.length);
    bytes.set(pending, 0);
    bytes.set(incoming, pending.length);

    let out = '';
    let i = 0;
    const len = bytes.length;
    while (i < len) {
      const b0 = bytes[i];
      if (b0 < 0x80) {
        out += String.fromCharCode(b0);
        i += 1;
      } else if ((b0 & 0xe0) === 0xc0) {
        if (i + 1 >= len) break;
        const cp = ((b0 & 0x1f) << 6) | (bytes[i + 1] & 0x3f);
        out += String.fromCharCode(cp);
        i += 2;
      } else if ((b0 & 0xf0) === 0xe0) {
        if (i + 2 >= len) break;
        const cp =
          ((b0 & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f);
        out += String.fromCharCode(cp);
        i += 3;
      } else if ((b0 & 0xf8) === 0xf0) {
        if (i + 3 >= len) break;
        const cp =
          ((b0 & 0x07) << 18) |
          ((bytes[i + 1] & 0x3f) << 12) |
          ((bytes[i + 2] & 0x3f) << 6) |
          (bytes[i + 3] & 0x3f);
        out += String.fromCodePoint(cp);
        i += 4;
      } else {
        // Invalid byte: skip safely
        i += 1;
      }
    }
    pending = bytes.slice(i);
    return out;
  }

  return { decode };
}

function streamTriageChat(payload, onDelta, onResult, onError, allowRetry = true) {
  let settled = false;
  let resultEmitted = false;
  let buffer = '';
  const utf8Decoder = createUtf8StreamDecoder();

  const finishError = (err) => {
    if (settled) return;
    settled = true;
    if (onError) onError(err);
  };

  const finishResult = (parsed) => {
    if (resultEmitted) return;
    resultEmitted = true;
    settled = true;
    if (onResult) onResult(parsed);
  };

  const processBuffer = () => {
    // Normalize CRLF -> LF so "\r\n\r\n" SSE delimiters never drop frames
    buffer = buffer.replace(/\r\n/g, '\n');
    const blocks = buffer.split('\n\n');
    buffer = blocks.pop() || '';
    blocks.forEach((block) => {
      const lines = block.split('\n');
      let eventType = 'message';
      let dataStr = '';
      lines.forEach((line) => {
        if (line.startsWith('event:')) eventType = line.slice(6).trim();
        if (line.startsWith('data:')) {
          dataStr += (dataStr ? '\n' : '') + line.slice(5).trim();
        }
      });
      if (!dataStr) return;
      try {
        const parsed = JSON.parse(dataStr);
        if (eventType === 'delta' && onDelta) onDelta(parsed);
        if (eventType === 'result') finishResult(parsed);
        if (eventType === 'error') finishError(new Error(parsed.error || 'stream error'));
      } catch (e) {
        // Ignore malformed non-JSON frame
      }
    });
  };

  const reqTask = wx.request({
    url: `${getBaseUrl()}/v1/triage/chat`,
    method: 'POST',
    enableChunked: true,
    timeout: 25000,
    data: Object.assign({}, payload, { stream: true }),
    header: Object.assign(
      { 'Content-Type': 'application/json', Accept: 'text/event-stream' },
      getAuthHeader()
    ),
    success(res) {
      if (res.statusCode === 401 && allowRetry) {
        tryRefreshAuth()
          .then(() => {
            streamTriageChat(payload, onDelta, onResult, onError, false);
          })
          .catch(finishError);
        return;
      }
      if (res.statusCode < 200 || res.statusCode >= 300) {
        const err = new Error((res.data && res.data.error) || `HTTP ${res.statusCode}`);
        err.statusCode = res.statusCode;
        finishError(err);
        return;
      }
      // Flush any trailing block in buffer
      if (buffer.trim()) {
        buffer += '\n\n';
        processBuffer();
      }
      if (!settled) {
        finishError(new Error('流式响应提前结束，未收到分诊结果'));
      }
    },
    fail(err) {
      finishError(err);
    },
    complete() {
      // Safety net: never allow caller UI to remain permanently stuck in loading=true
      if (!settled && !allowRetry) {
        finishError(new Error('连接已中断，请点击重试'));
      }
    },
  });

  if (reqTask && typeof reqTask.onChunkReceived === 'function') {
    reqTask.onChunkReceived((res) => {
      if (!res || !res.data) return;
      buffer += utf8Decoder.decode(res.data);
      processBuffer();
    });
  }
  return reqTask;
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
  createUtf8StreamDecoder,
  getCurrentEpidemics,
  getPatientMemory,
  updatePatientMemory,
  deletePatientMemory,
};
