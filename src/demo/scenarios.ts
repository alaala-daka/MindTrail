import type { TrailEvent, Payloads } from '../domain/events'
export type Scenario = { id: string; title: string; initialCount: number; initialNodeId: string; events: readonly TrailEvent[] }
function createEvents(session: string, date: string) {
  const events: TrailEvent[] = []
  function add<K extends keyof Payloads>(type: K, payload: Payloads[K] & { quote?: string; context?: string }) {
    const seq = events.length + 1
    events.push({ v: 1, session, seq, ts: new Date(Date.parse(date) + (seq - 1) * 1000).toISOString(), type, payload } as TrailEvent)
  }
  return { events, add }
}
const auth = createEvents('auth', '2026-10-13T09:41:00Z')
auth.add('task.start', { title: '实现用户认证', goal: '登录、续期及受保护路由', project: 'demo-auth' })
auth.add('capability.intent', { id: 'login', title: '登录鉴权' })
auth.add('api.expose', { id: 'login-api', capabilityId: 'login', kind: 'http', signature: 'POST /api/login', summary: '校验凭据并签发令牌', quote: '先校验邮箱和密码，再签发令牌' })
auth.add('file.change', { id: 'handler', capabilityId: 'login', path: 'src/auth/login.ts', action: 'modify', stats: { add: 142, del: 18 }, purpose: '登录校验与令牌签发编排' })
auth.add('file.change', { id: 'password', capabilityId: 'login', path: 'src/auth/password.ts', action: 'create', stats: { add: 56, del: 0 }, purpose: '校验密码' })
auth.add('call.edge', { from: 'login-api', to: 'handler', kind: 'calls' })
auth.add('call.edge', { from: 'handler', to: 'password', kind: 'imports' })
auth.add('capability.done', { id: 'login' })
auth.add('capability.intent', { id: 'session', title: '会话管理' })
auth.add('file.change', { id: 'session-file', capabilityId: 'session', path: 'src/auth/session.ts', action: 'create', stats: { add: 98, del: 0 } })
auth.add('capability.intent', { id: 'middleware', title: '权限中间件' })
auth.add('api.expose', { id: 'refresh-api', capabilityId: 'session', kind: 'http', signature: 'POST /api/refresh' })
auth.add('capability.done', { id: 'session' })
auth.add('file.change', { id: 'middleware-file', capabilityId: 'middleware', path: 'src/auth/require-user.ts', action: 'create', stats: { add: 26, del: 0 } })
auth.add('capability.done', { id: 'middleware' })
auth.add('task.done', { summary: '认证任务完成' })
const orders = createEvents('orders', '2026-10-14T09:41:00Z')
orders.add('task.start', { title: '订单导出功能', goal: '导出订单 CSV', project: 'demo-orders' })
orders.add('capability.intent', { id: 'export', title: '导出订单' })
orders.add('api.expose', { id: 'export-api', capabilityId: 'export', kind: 'http', signature: 'GET /api/orders/export' })
orders.add('file.change', { id: 'handler', capabilityId: 'export', path: 'src/orders/export.ts', action: 'create', stats: { add: 32, del: 0 } })
orders.add('call.edge', { from: 'export-api', to: 'handler', kind: 'calls' })
orders.add('file.change', { id: 'csv-config', path: 'src/orders/csv-config.ts', action: 'create', stats: { add: 12, del: 0 } })
orders.add('capability.done', { id: 'export' })
orders.add('task.done', {})
export const scenarios: readonly Scenario[] = [
  { id: 'auth', title: '实现用户认证', initialCount: 11, initialNodeId: 'login-api', events: auth.events },
  { id: 'orders', title: '订单导出功能', initialCount: 8, initialNodeId: 'root', events: orders.events },
]
