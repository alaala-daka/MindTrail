import type { TrailEvent } from './events'
export type GraphMode = 'hierarchy' | 'calls'
export type TrailNode = { id: string; kind: 'task' | 'capability' | 'api' | 'file' | 'group'; title: string; description?: string; parentId?: string; state?: 'planned' | 'active' | 'done'; firstSeq: number; litAt?: string; events: readonly TrailEvent[] }
export type TrailEdge = { id: string; from: string; to: string; kind: 'contains' | 'calls' | 'imports' }
export type SessionProjection = { id: string; status: 'running' | 'done'; lastSeq: number; nodes: ReadonlyMap<string, TrailNode>; callEdges: readonly TrailEdge[] }
export type VisibleGraph = { nodes: readonly TrailNode[]; edges: readonly TrailEdge[] }
export function projectSession(events: readonly TrailEvent[], appliedCount: number): SessionProjection {
  const nodes = new Map<string, TrailNode>()
  const callEdges: TrailEdge[] = []
  let id = '', status: SessionProjection['status'] = 'running', lastSeq = 0
  for (const event of events.slice(0, Math.max(0, appliedCount))) {
    id = event.session; lastSeq = event.seq
    const update = (nodeId: string, props: Partial<TrailNode>) => {
      const node = nodes.get(nodeId)
      if (node) nodes.set(nodeId, { ...node, ...props, events: [...node.events, event] })
    }
    switch (event.type) {
      case 'task.start': nodes.set('root', { id: 'root', kind: 'task', title: event.payload.title, description: event.payload.goal, firstSeq: event.seq, events: [event] }); break
      case 'capability.intent': nodes.set(event.payload.id, { id: event.payload.id, kind: 'capability', title: event.payload.title, description: event.payload.summary, parentId: 'root', state: 'planned', firstSeq: event.seq, events: [event] }); break
      case 'capability.done': update(event.payload.id, { state: 'done', litAt: event.ts, ...(event.payload.summary ? { description: event.payload.summary } : {}) }); break
      case 'api.expose':
      case 'file.change': {
        const payload = event.payload
        const validParent = payload.capabilityId && nodes.get(payload.capabilityId)?.kind === 'capability'
        const parentId = validParent ? payload.capabilityId! : 'unassigned'
        if (!validParent && !nodes.has('unassigned')) nodes.set('unassigned', { id: 'unassigned', kind: 'group', title: '未归属', parentId: 'root', firstSeq: event.seq, events: [] })
        const node: TrailNode = event.type === 'api.expose'
          ? { id: event.payload.id, kind: 'api', title: event.payload.signature, description: event.payload.summary, parentId, firstSeq: event.seq, events: [event] }
          : { id: event.payload.id, kind: 'file', title: event.payload.path, description: event.payload.purpose, parentId, firstSeq: event.seq, events: [event] }
        nodes.set(node.id, node)
        if (event.type === 'file.change' && validParent && nodes.get(parentId)?.state === 'planned') {
          const parent = nodes.get(parentId)!
          nodes.set(parentId, { ...parent, state: 'active' })
        }
        break
      }
      case 'call.edge': callEdges.push({ id: `${event.payload.from}:${event.payload.kind}:${event.payload.to}`, ...event.payload }); break
      case 'task.done': status = 'done'; update('root', {}); break
    }
  }
  return { id, status, lastSeq, nodes, callEdges }
}
export function projectGraph(session: SessionProjection, mode: GraphMode): VisibleGraph {
  const nodes = [...session.nodes.values()].filter(node => mode === 'hierarchy' || node.kind === 'api' || node.kind === 'file')
  const visible = new Set(nodes.map(node => node.id))
  const edges: TrailEdge[] = mode === 'hierarchy'
    ? nodes.filter(node => node.parentId && visible.has(node.parentId)).map(node => ({ id: `contains:${node.parentId}:${node.id}`, from: node.parentId!, to: node.id, kind: 'contains' }))
    : session.callEdges.filter(edge => visible.has(edge.from) && visible.has(edge.to))
  return { nodes, edges }
}
