import { expect, it } from 'vitest'
import { scenarios } from '../demo/scenarios'
import { projectSession } from '../domain/project'
import { appReducer } from './appState'
import { createWorkspace } from './state'
import type { Snapshot } from './gateway'

it('commits identity, graph input and selection together when switching a same-named task', () => {
  const demo = scenarios[0]
  const old = { workspace: createWorkspace(demo), snapshot: null }
  const snapshot: Snapshot = { storeId: 'store', selected: demo.id, sessions: [], events: [], watermark: 0, status: null, error: null, changedStore: false }
  const switched = appReducer(old, { type: 'snapshot', snapshot })
  expect(switched.workspace.identity).toEqual({ kind: 'real', storeId: 'store', session: demo.id })
  expect(projectSession(switched.workspace.scenario.events, switched.workspace.appliedCount).nodes.size).toBe(0)
  expect(switched.workspace.insightNodeId).toBeNull()
  const restored = appReducer(switched, { type: 'workspace', action: { type: 'switchTask', scenario: demo } })
  expect(restored.workspace.identity).toEqual({ kind: 'demo', id: demo.id })
  expect(projectSession(restored.workspace.scenario.events, restored.workspace.appliedCount).nodes.size).toBeGreaterThan(0)
})

it('preserves view and node for an increment, and clears them on store change', () => {
  const demo = scenarios[0]
  const snapshot: Snapshot = { storeId: 'one', selected: demo.id, sessions: [], events: [...demo.events.slice(0, 3)], watermark: 3, status: 'open', error: null, changedStore: false }
  let state = appReducer({ workspace: createWorkspace(demo), snapshot: null }, { type: 'snapshot', snapshot })
  state = appReducer(state, { type: 'workspace', action: { type: 'selectNode', nodeId: 'login-api' } })
  state = appReducer(state, { type: 'workspace', action: { type: 'switchMode', mode: 'calls' } })
  state = appReducer(state, { type: 'snapshot', snapshot: { ...snapshot, events: [...demo.events.slice(0, 4)], watermark: 4 } })
  expect(state.workspace).toMatchObject({ mode: 'calls', selectedId: 'login-api' })
  state = appReducer(state, { type: 'snapshot', snapshot: { ...snapshot, storeId: 'two', events: [], watermark: 0 } })
  expect(state.workspace).toMatchObject({ mode: 'hierarchy', selectedId: 'root', appliedCount: 0 })
})
