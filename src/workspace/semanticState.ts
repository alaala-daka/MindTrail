import type { SemanticContext, SemanticPreview, SemanticResult } from './semantic'

export type SemanticStatus = 'idle' | 'preparing' | 'preview' | 'generating' | 'ready' | 'error'
export type SemanticState = { status: SemanticStatus; contextKey: string; requestId: string; generation: number; preview?: SemanticPreview; result?: SemanticResult; error?: string }
export type SemanticAction =
  | { type: 'invalidate'; contextKey: string; generation: number }
  | { type: 'start'; status: 'preparing' | 'generating'; contextKey: string; requestId: string; generation: number }
  | { type: 'prepared'; contextKey: string; requestId: string; generation: number; preview: SemanticPreview }
  | { type: 'resolved'; contextKey: string; requestId: string; generation: number; result: SemanticResult }
  | { type: 'failed'; contextKey: string; requestId: string; generation: number; error: string }

export function contextKey(context: SemanticContext): string { return JSON.stringify([context.store_id, context.session_id, context.node_id, context.prefix_seq]) }
export function acceptResponse(currentKey: string, responseKey: string): boolean {
  return currentKey === responseKey
}
export function semanticReducer(state: SemanticState, action: SemanticAction): SemanticState {
  if (action.type === 'invalidate') return { status: 'idle', contextKey: action.contextKey, requestId: '', generation: action.generation }
  if (action.type === 'start') {
    if (action.contextKey !== state.contextKey || action.generation !== state.generation) return state
    return { ...state, status: action.status, requestId: action.requestId, error: undefined }
  }
  if (action.contextKey !== state.contextKey || action.requestId !== state.requestId || action.generation !== state.generation) return state
  if (action.type === 'prepared') return contextKey(action.preview.context) === state.contextKey ? { ...state, status: 'preview', preview: action.preview } : state
  if (action.type === 'resolved') return contextKey(action.result.context) === state.contextKey ? { ...state, status: 'ready', result: action.result, preview: undefined } : state
  return { ...state, status: 'error', error: action.error, preview: undefined, result: undefined }
}
