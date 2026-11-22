import { expect, it } from 'vitest'
import { scenarios } from '../demo/scenarios'
import { sampleInsights } from '../demo/insights'
import { projectSession } from '../domain/project'
import { getNodeEvidence, resolveCitation } from './evidence'
it('isolates real evidence when demo identifiers collide', () => {
  const session = projectSession(scenarios[0].events, 11)
  expect(getNodeEvidence(session, 'login-api', 'real').every(item => item.kind === 'quote')).toBe(true)
  expect(getNodeEvidence(session, 'login-api', 'demo').some(item => item.kind === 'diff')).toBe(true)
})
it('resolves only released citations belonging to this insight and task', () => {
  const [auth, orders] = scenarios
  const insight = sampleInsights[0]
  const [quote] = insight.sections[0].citations
  const [diff, src] = insight.sections[1].citations
  const initial = projectSession(auth.events, 11)
  expect(resolveCitation(initial, insight, diff, 'real')).toBeNull()
  expect(resolveCitation(initial, insight, src, 'real')).toBeNull()
  expect(resolveCitation(initial, insight, quote, 'demo')?.text).toContain('密码')
  expect(resolveCitation(initial, insight, diff, 'demo')?.path).toBe('src/auth/login.ts')
  expect(resolveCitation(initial, insight, src, 'demo')?.firstLine).toBe(42)
  expect(resolveCitation(projectSession(auth.events, 3), insight, diff, 'demo')).toBeNull()
  expect(resolveCitation(initial, insight, { ...diff, id: 'missing' }, 'demo')).toBeNull()
  expect(resolveCitation(projectSession(orders.events, 8), insight, diff, 'demo')).toBeNull()
})

it('does not resolve another task’s evidence for a shared node ID', () => {
  const [auth, orders] = scenarios
  const orderEvents = orders.events.map(event =>
    event.type === 'file.change' && event.payload.id === 'handler'
      ? { ...event, payload: { ...event.payload, quote: '订单导出处理程序' } }
      : event,
  )
  const authSession = projectSession(auth.events, 11)
  const orderSession = projectSession(orderEvents, 8)
  const ref = { kind: 'quote', nodeId: 'handler', id: 'orders:4' } as const
  const insight = {
    sessionId: 'auth',
    nodeId: 'handler',
    sections: [{ title: '处理程序', text: '', citations: [ref], relatedNodeIds: [] }],
  }

  expect(authSession.nodes.has('handler')).toBe(true)
  expect(orderSession.nodes.has('handler')).toBe(true)
  expect(getNodeEvidence(orderSession, 'handler', 'demo')).toContainEqual(expect.objectContaining(ref))
  expect(resolveCitation(orderSession, { ...insight, sessionId: 'orders' }, ref, 'demo')?.text).toBe('订单导出处理程序')
  expect(resolveCitation(orderSession, insight, ref, 'demo')).toBeNull()
})
