export type Payloads = {
  'task.start': { title: string; goal: string; project: string }
  'capability.intent': { id: string; title: string; summary?: string }
  'capability.done': { id: string; summary?: string }
  'api.expose': { id: string; capabilityId?: string; kind: 'http' | 'cli' | 'fn'; signature: string; summary?: string }
  'file.change': { id: string; capabilityId?: string; path: string; action: 'create' | 'modify' | 'delete'; purpose?: string; stats: { add: number; del: number } }
  'call.edge': { from: string; to: string; kind: 'calls' | 'imports' }
  'task.done': { summary?: string }
}
export type TrailEvent = { [K in keyof Payloads]: { v: 1; session: string; seq: number; ts: string; type: K; payload: Payloads[K] & { quote?: string; context?: string } } }[keyof Payloads]
