import type { Snapshot } from './gateway'
import { workspaceReducer, type WorkspaceAction, type WorkspaceState } from './state'

export type AppState = { workspace: WorkspaceState; snapshot: Snapshot | null }
export type AppAction = { type: 'snapshot'; snapshot: Snapshot } | { type: 'workspace'; action: WorkspaceAction }
export function appReducer(state: AppState, action: AppAction): AppState {
  if (action.type === 'workspace') return { ...state, workspace: workspaceReducer(state.workspace, action.action) }
  const snapshot = action.snapshot
  if (!snapshot.selected || !snapshot.storeId) return { ...state, snapshot }
  const summary = snapshot.sessions.find(item => item.session === snapshot.selected)
  const identity = { kind: 'real' as const, storeId: snapshot.storeId, session: snapshot.selected }
  const scenario = { id: snapshot.selected, title: summary?.title ?? snapshot.selected, initialCount: 0, initialNodeId: 'root', events: snapshot.events }
  return { snapshot, workspace: workspaceReducer(state.workspace, { type: 'updateReal', identity, scenario }) }
}
