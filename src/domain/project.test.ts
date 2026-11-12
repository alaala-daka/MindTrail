import { describe, expect, it } from 'vitest'
import { scenarios } from '../demo/scenarios'
import { projectGraph, projectSession } from './project'

describe('session projection', () => {
  const [auth, orders] = scenarios
  it('tracks capability stages across prefixes', () => {
    expect(projectSession(auth.events, 2).nodes.get('login')?.state).toBe('planned')
    expect(projectSession(auth.events, 4).nodes.get('login')?.state).toBe('active')
    expect(projectSession(auth.events, 8).nodes.get('login')?.state).toBe('done')
    expect(projectSession(auth.events, 11).nodes.get('session-file')?.description).toBeUndefined()
    expect(projectSession(auth.events, 16).status).toBe('done')
  })
  it('produces deterministic graphs and directed call edges', () => {
    const result = projectSession(auth.events, 16)
    expect(result).toEqual(projectSession(auth.events, 16))
    expect(projectGraph(result, 'calls').edges.map(({from, to, kind}) => [from, to, kind])).toEqual([['login-api', 'handler', 'calls'], ['handler', 'password', 'imports']])
    expect(projectGraph(projectSession(auth.events, 0), 'hierarchy')).toEqual({ nodes: [], edges: [] })
  })
  it('keeps task-local identifiers and groups unassigned files', () => {
    const result = projectSession(orders.events, 8)
    expect(result.nodes.get('handler')?.title).toBe('src/orders/export.ts')
    expect(result.nodes.get('csv-config')?.parentId).toBe('unassigned')
    expect(result.nodes.get('unassigned')?.parentId).toBe('root')
  })
})

it('handles prefix boundaries and invalid capability references', () => {
  const [auth] = scenarios
  const empty = projectSession(auth.events, 0)
  expect(empty.lastSeq).toBe(0)
  expect(empty.nodes.size).toBe(0)
  const file = auth.events[3]
  if (file.type !== 'file.change') throw new Error('Expected file change')
  const invalid = [...auth.events.slice(0, 3), { ...file, payload: { ...file.payload, capabilityId: 'login-api' } }]
  const projection = projectSession(invalid, invalid.length)
  expect(projection.nodes.get('handler')?.parentId).toBe('unassigned')
  expect(projection.nodes.get('login')?.state).toBe('planned')
  expect(projectGraph(projection, 'hierarchy').edges).toContainEqual({ id: 'contains:unassigned:handler', from: 'unassigned', to: 'handler', kind: 'contains' })
  expect(projectGraph(projection, 'calls').edges).toEqual([])
})

it('filters call edges with invisible endpoints and retains visible nodes', () => {
  const session = projectSession(scenarios[0].events, 8)
  const graph = projectGraph({ ...session, callEdges: [...session.callEdges, { id: 'hidden', from: 'login', to: 'handler', kind: 'calls' }] }, 'calls')
  expect(graph.nodes.map(node => node.id)).toEqual(['login-api', 'handler', 'password'])
  expect(graph.edges.map(edge => edge.id)).toEqual(['login-api:calls:handler', 'handler:imports:password'])
})

it('preserves planned state, completion time, and identifiable quoted evidence', () => {
  const [auth] = scenarios
  expect(projectSession(auth.events, 3).nodes.get('login')?.state).toBe('planned')
  expect(projectSession(auth.events, 8).nodes.get('login')?.litAt).toBe(auth.events[7].ts)
  const evidence = projectSession(auth.events, 3).nodes.get('login-api')?.events.find(event => `${event.session}:${event.seq}` === 'auth:3')
  expect(evidence?.payload.quote).toBe('先校验邮箱和密码，再签发令牌')
})

it('isolates repeated projections across tasks with shared node IDs', () => {
  const [auth, orders] = scenarios
  const first = projectSession(auth.events, 16)
  const other = projectSession(orders.events, 8)
  const again = projectSession(auth.events, 16)
  expect(first.nodes.get('handler')?.title).toBe('src/auth/login.ts')
  expect(other.nodes.get('handler')?.title).toBe('src/orders/export.ts')
  expect(again).toEqual(first)
  expect(first.nodes.get('handler')?.title).toBe('src/auth/login.ts')
})
