import type { GraphFocus } from '../graph/GraphCanvas'
import type { Scenario } from '../demo/scenarios'
import { projectGraph, projectSession, type GraphMode } from '../domain/project'
export type FactTab = 'quotes' | 'diff' | 'calls' | 'meta'
export type WorkspaceState = { scenario: Scenario; appliedCount: number; playing: boolean; runId: number; mode: GraphMode; selectedId: string | null; hierarchySelectionId: string | null; factTab: FactTab; factsExpanded: boolean; insightNodeId: string | null; focus: GraphFocus | null; focusSeq: number }
export type WorkspaceAction =
  | { type: 'locateNode'; nodeId: string; closeOverlay: boolean }
  | { type: 'revealEvidence'; nodeId: string; tab: 'quotes' | 'diff'; closeOverlay: boolean }
  | { type: 'switchTask'; scenario: Scenario } | { type: 'restart' } | { type: 'togglePlayback' }
  | { type: 'tick'; runId: number; expectedCount: number } | { type: 'switchMode'; mode: GraphMode }
  | { type: 'focusNode'; nodeId: string; moveKeyboardFocus: boolean }
  | { type: 'selectNode'; nodeId: string } | { type: 'openInsight'; nodeId: string } | { type: 'closeInsight' }
  | { type: 'setFactTab'; tab: FactTab } | { type: 'toggleFacts' }
export function createWorkspace(scenario: Scenario): WorkspaceState {
  return { scenario, appliedCount: scenario.initialCount, playing: false, runId: 0, mode: 'hierarchy', selectedId: scenario.initialNodeId, hierarchySelectionId: null, factTab: 'quotes', factsExpanded: true, insightNodeId: null, focus: null, focusSeq: 0 }
}
export function workspaceReducer(state: WorkspaceState, action: WorkspaceAction): WorkspaceState {
  switch (action.type) {
    case 'switchTask': return action.scenario.id === state.scenario.id ? state : { ...createWorkspace(action.scenario), runId: state.runId + 1 }
    case 'restart': return { ...state, appliedCount: 1, playing: true, runId: state.runId + 1, mode: 'hierarchy', selectedId: 'root', hierarchySelectionId: null, insightNodeId: null, focus: null }
    case 'togglePlayback': return state.appliedCount >= state.scenario.events.length ? state : { ...state, playing: !state.playing, runId: state.runId + 1 }
    case 'tick': {
      if (!state.playing || state.runId !== action.runId || state.appliedCount !== action.expectedCount) return state
      const appliedCount = Math.min(state.appliedCount + 1, state.scenario.events.length)
      return { ...state, appliedCount, playing: appliedCount < state.scenario.events.length }
    }
    case 'switchMode': {
      if (action.mode === state.mode) return state
      const visible = new Set(projectGraph(projectSession(state.scenario.events, state.appliedCount), action.mode).nodes.map(node => node.id))
      const selectedId = state.selectedId && visible.has(state.selectedId) ? state.selectedId : action.mode === 'hierarchy' && state.hierarchySelectionId && visible.has(state.hierarchySelectionId) ? state.hierarchySelectionId : null
      return { ...state, mode: action.mode, selectedId, focusSeq: state.focusSeq + 1, focus: selectedId ? { requestId: state.focusSeq + 1, nodeId: selectedId, moveKeyboardFocus: false } : null, hierarchySelectionId: action.mode === 'calls' && !selectedId ? state.selectedId : state.hierarchySelectionId }
    }
    case 'locateNode': {
      const session = projectSession(state.scenario.events, state.appliedCount)
      if (!session.nodes.has(action.nodeId)) return state
      const visible = projectGraph(session, state.mode).nodes.some(node => node.id === action.nodeId)
      return { ...state, mode: visible ? state.mode : 'hierarchy', selectedId: action.nodeId, factsExpanded: true, insightNodeId: action.closeOverlay ? null : state.insightNodeId, focusSeq: state.focusSeq + 1, focus: { requestId: state.focusSeq + 1, nodeId: action.nodeId, moveKeyboardFocus: true } }
    }
    case 'revealEvidence': {
      const session = projectSession(state.scenario.events, state.appliedCount)
      if (!session.nodes.has(action.nodeId)) return state
      const visible = projectGraph(session, state.mode).nodes.some(node => node.id === action.nodeId)
      return { ...state, mode: visible ? state.mode : 'hierarchy', selectedId: action.nodeId, factsExpanded: true, factTab: action.tab, insightNodeId: action.closeOverlay ? null : state.insightNodeId, focus: null }
    }
    case 'focusNode': return { ...state, focusSeq: state.focusSeq + 1, focus: { requestId: state.focusSeq + 1, nodeId: action.nodeId, moveKeyboardFocus: action.moveKeyboardFocus } }
    case 'selectNode': return { ...state, selectedId: action.nodeId, factsExpanded: true }
    case 'openInsight': return { ...state, selectedId: action.nodeId, insightNodeId: action.nodeId, factsExpanded: true }
    case 'closeInsight': return { ...state, insightNodeId: null, focus: null }
    case 'setFactTab': return { ...state, factTab: action.tab }
    case 'toggleFacts': return { ...state, factsExpanded: !state.factsExpanded }
  }
}
