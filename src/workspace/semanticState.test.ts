import { expect, it } from 'vitest'
import { acceptResponse, contextKey, semanticReducer, type SemanticState } from './semanticState'

it('rejects responses from another store, session, node, prefix or closed generation', () => {
  const key = JSON.stringify(['store-a', 'session-a', 'node-a', 7, 1])
  expect(acceptResponse(key, key)).toBe(true)
  for (const other of [
    ['store-b', 'session-a', 'node-a', 7, 1],
    ['store-a', 'session-b', 'node-a', 7, 1],
    ['store-a', 'session-a', 'node-b', 7, 1],
    ['store-a', 'session-a', 'node-a', 6, 1],
    ['store-a', 'session-a', 'node-a', 7, 2],
  ]) expect(acceptResponse(key, JSON.stringify(other))).toBe(false)
})

it('ignores late results after cancellation or context changes', () => {
  const context = { store_id: 'store', session_id: 'session', node_id: 'node', prefix_seq: 3 }
  const key = contextKey(context)
  const initial: SemanticState = { status: 'idle', contextKey: key, requestId: '', generation: 1 }
  const running = semanticReducer(initial, { type: 'start', status: 'generating', contextKey: key, requestId: 'request', generation: 1 })
  const result = { context, provider: 'mock', model: 'm', prompt_version: '1', sections: [] }
  expect(semanticReducer(running, { type: 'resolved', contextKey: key, requestId: 'request', generation: 1, result }).status).toBe('ready')
  const cancelled = semanticReducer(running, { type: 'invalidate', contextKey: key, generation: 2 })
  expect(semanticReducer(cancelled, { type: 'resolved', contextKey: key, requestId: 'request', generation: 1, result })).toEqual(cancelled)
  for (const changed of [{ ...context, store_id: 'other' }, { ...context, session_id: 'other' }, { ...context, node_id: 'other' }, { ...context, prefix_seq: 4 }]) {
    expect(semanticReducer(running, { type: 'resolved', contextKey: key, requestId: 'request', generation: 1, result: { ...result, context: changed } })).toEqual(running)
  }
  expect(semanticReducer(running, { type: 'resolved', contextKey: key, requestId: 'other', generation: 1, result })).toEqual(running)
})
