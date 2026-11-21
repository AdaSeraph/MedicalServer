import http from 'k6/http';
import { check, sleep } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://127.0.0.1:8080';

export const options = {
  stages: [
    { duration: '15s', target: 50 },
    { duration: '30s', target: 200 },
    { duration: '15s', target: 0 },
  ],
  thresholds: {
    http_req_failed: ['rate<0.01'],
    http_req_duration: ['p(95)<800', 'p(99)<1500'],
  },
};

export function setup() {
  const healthRes = http.get(`${BASE_URL}/health`);
  check(healthRes, {
    'health is 200': (r) => r.status === 200,
  });

  const authRes = http.post(
    `${BASE_URL}/v1/auth/wechat`,
    JSON.stringify({ code: 'k6_load_user_001' }),
    { headers: { 'Content-Type': 'application/json' } }
  );
  check(authRes, {
    'wechat auth is 200': (r) => r.status === 200,
  });
  const auth = authRes.json();
  return { token: auth.token, patientId: auth.patient_id };
}

export default function (data) {
  const headers = {
    'Content-Type': 'application/json',
    Authorization: `Bearer ${data.token}`,
  };

  // 1. Hybrid knowledge search
  const searchRes = http.post(
    `${BASE_URL}/v1/knowledge/search`,
    JSON.stringify({
      query: '高热 肌肉酸痛 咽痛',
      region_code: 'CN-110000',
      seasonal_boost: true,
      top_k: 5,
    }),
    { headers }
  );
  check(searchRes, {
    'knowledge search 200': (r) => r.status === 200,
  });

  // 2. ECS Triage chat (normal + emergency mix)
  const isEmergency = __ITER % 20 === 0;
  const utterance = isEmergency
    ? '突然胸痛，伴随大汗和左臂放射痛'
    : '发烧39度，咽痛干咳两天';

  const triageRes = http.post(
    `${BASE_URL}/v1/triage/chat`,
    JSON.stringify({
      session_id: `k6_sess_${__VU}_${__ITER}`,
      patient_id: data.patientId,
      region_code: 'CN-110000',
      utterance,
      stream: false,
    }),
    { headers }
  );
  check(triageRes, {
    'triage chat 200': (r) => r.status === 200,
  });

  sleep(0.2);
}
