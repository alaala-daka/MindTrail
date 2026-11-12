import { describe, expect, it } from 'vitest'
import { scenarios } from '../demo/scenarios'
import { createWorkspace, workspaceReducer } from './state'

describe('workspace playback', () => {
  const [auth, orders] = scenarios
  it('locates a node hidden from the current graph in hierarchy view', () => {
    const calls = workspaceReducer(createWorkspace(auth), { type: 'switchMode', mode: 'calls' })
    const located = workspaceReducer(calls, { type: 'locateNode', nodeId: 'login', closeOverlay: false })
    expect(located).toMatchObject({ mode: 'hierarchy', selectedId: 'login', factsExpanded: true, focus: { nodeId: 'login', moveKeyboardFocus: true } })
    expect(workspaceReducer(located, { type: 'locateNode', nodeId: 'missing', closeOverlay: false })).toBe(located)
  })
  it('reveals evidence atomically while preserving playback and overlay as requested', () => {
    let state = workspaceReducer(createWorkspace(auth), { type: 'switchMode', mode: 'calls' })
    state = { ...state, playing: true, runId: 42, insightNodeId: 'login-api', factsExpanded: false, focus: { requestId: 1, nodeId: 'root', moveKeyboardFocus: true } }
    const visible = workspaceReducer(state, { type: 'revealEvidence', nodeId: 'login-api', tab: 'quotes', closeOverlay: false })
    expect(visible).toMatchObject({ mode: 'calls', selectedId: 'login-api', factTab: 'quotes', factsExpanded: true, insightNodeId: 'login-api', focus: null, playing: true, runId: 42 })
    const hidden = workspaceReducer(state, { type: 'revealEvidence', nodeId: 'login', tab: 'diff', closeOverlay: true })
    expect(hidden).toMatchObject({ mode: 'hierarchy', selectedId: 'login', factTab: 'diff', factsExpanded: true, insightNodeId: null, focus: null, playing: true, runId: 42 })
    expect(workspaceReducer(state, { type: 'revealEvidence', nodeId: 'missing', tab: 'diff', closeOverlay: true })).toBe(state)
  })
  it('ignores nodes that have not yet appeared during playback', () => {
    const replay = workspaceReducer(createWorkspace(auth), { type: 'restart' })
    expect(workspaceReducer(replay, { type: 'locateNode', nodeId: 'login-api', closeOverlay: true })).toBe(replay)
    expect(workspaceReducer(replay, { type: 'revealEvidence', nodeId: 'login-api', tab: 'quotes', closeOverlay: true })).toBe(replay)
  })
  it('keeps the insight pinned when selecting another node and closes it for overlay location', () => {
    const opened = workspaceReducer(createWorkspace(auth), { type: 'openInsight', nodeId: 'login-api' })
    const selected = workspaceReducer(opened, { type: 'selectNode', nodeId: 'login' })
    expect(selected.insightNodeId).toBe('login-api')
    const located = workspaceReducer(selected, { type: 'locateNode', nodeId: 'login-api', closeOverlay: true })
    expect(located).toMatchObject({ insightNodeId: null, selectedId: 'login-api', focus: { nodeId: 'login-api', moveKeyboardFocus: true } })
  })
  it('starts with the preset and cancels ticks across tasks and pauses', () => {
    const initial = createWorkspace(auth)
    expect(initial).toMatchObject({ appliedCount: 11, selectedId: 'login-api', playing: false })
    const replay = workspaceReducer(initial, { type: 'restart' })
    const tick = { type: 'tick', runId: replay.runId, expectedCount: 1 } as const
    expect(workspaceReducer(replay, tick).appliedCount).toBe(2)
    const switched = workspaceReducer(replay, { type: 'switchTask', scenario: orders })
    expect(workspaceReducer(switched, tick)).toBe(switched)
    expect(switched.appliedCount).toBe(8)
    const paused = workspaceReducer(replay, { type: 'togglePlayback' })
    expect(workspaceReducer(paused, tick)).toBe(paused)
  })
  it('ignores a repeated tick in the same playback run', () => {
    const replay = workspaceReducer(createWorkspace(auth), { type: 'restart' })
    const tick = { type: 'tick', runId: replay.runId, expectedCount: replay.appliedCount } as const
    const advanced = workspaceReducer(replay, tick)

    expect(advanced.appliedCount).toBe(2)
    expect(advanced.playing).toBe(true)
    expect(workspaceReducer(advanced, tick)).toBe(advanced)
  })
  it('stops at the end and restores a hidden hierarchy selection', () => {
    let state = workspaceReducer(createWorkspace(auth), { type: 'selectNode', nodeId: 'login' })
    state = workspaceReducer(state, { type: 'switchMode', mode: 'calls' })
    expect(state.selectedId).toBeNull()
    state = workspaceReducer(state, { type: 'switchMode', mode: 'hierarchy' })
    expect(state.selectedId).toBe('login')
    state = workspaceReducer(state, { type: 'restart' })
    for (let i = 0; i < auth.events.length && state.playing; i++) state = workspaceReducer(state, { type: 'tick', runId: state.runId, expectedCount: state.appliedCount })
    expect(state.playing).toBe(false)
    expect(state.appliedCount).toBe(auth.events.length)
  })
  it('rejects old runs after pause and restart and resets task fields', () => {
    const [auth, orders] = scenarios
    const first = workspaceReducer(createWorkspace(auth), { type: 'restart' })
    const tick = { type: 'tick', runId: first.runId, expectedCount: 1 } as const
    const paused = workspaceReducer(first, { type: 'togglePlayback' })
    const resumed = workspaceReducer(paused, { type: 'togglePlayback' })
    expect(workspaceReducer(resumed, tick)).toBe(resumed)
    const restarted = workspaceReducer(resumed, { type: 'restart' })
    const resumedTick = { type: 'tick', runId: resumed.runId, expectedCount: resumed.appliedCount } as const
    expect(workspaceReducer(restarted, resumedTick)).toBe(restarted)
    const insight = workspaceReducer(restarted, { type: 'openInsight', nodeId: 'login' })
    const reset = workspaceReducer(insight, { type: 'restart' })
    expect(reset).toMatchObject({ selectedId: 'root', insightNodeId: null, appliedCount: 1, playing: true })
    expect(workspaceReducer(insight, { type: 'switchTask', scenario: auth })).toBe(insight)
    const changed = workspaceReducer(insight, { type: 'setFactTab', tab: 'diff' })
    const switched = workspaceReducer(changed, { type: 'switchTask', scenario: orders })
    expect(switched).toEqual({ ...createWorkspace(orders), runId: changed.runId + 1 })
  })


  it('issues distinct focus requests and clears pending requests on restart', () => {
    let state = createWorkspace(auth)
    state = workspaceReducer(state, { type: 'focusNode', nodeId: 'login-api', moveKeyboardFocus: true })
    const first = state.focus!.requestId
    state = workspaceReducer(state, { type: 'focusNode', nodeId: 'login-api', moveKeyboardFocus: true })
    expect(state.focus!.requestId).toBeGreaterThan(first)
        state = workspaceReducer(state, { type: 'closeInsight' })
    state = workspaceReducer(state, { type: 'focusNode', nodeId: 'login-api', moveKeyboardFocus: false })
    expect(state.focus!.requestId).toBeGreaterThan(first + 1)
    const beforeRestart = state.focus!.requestId
    state = workspaceReducer(state, { type: 'restart' })
    expect(state.focus).toBeNull()
    state = workspaceReducer(state, { type: 'focusNode', nodeId: 'root', moveKeyboardFocus: false })
    expect(state.focus!.requestId).toBeGreaterThan(beforeRestart)
  })})
