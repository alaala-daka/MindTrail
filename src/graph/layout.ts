import dagre from '@dagrejs/dagre'
import type { VisibleGraph } from '../domain/project'
export const NODE_WIDTH = 220
export const NODE_HEIGHT = 74
export function layoutGraph(graph: VisibleGraph): ReadonlyMap<string, { x: number; y: number }> {
  const g = new dagre.graphlib.Graph({ multigraph: true }).setGraph({ rankdir: 'LR', nodesep: 30, ranksep: 80 }).setDefaultEdgeLabel(() => ({}))
  ;[...graph.nodes].sort((a,b) => a.id.localeCompare(b.id)).forEach(n => g.setNode(n.id, { width: NODE_WIDTH, height: NODE_HEIGHT }))
  ;[...graph.edges].sort((a,b) => a.id.localeCompare(b.id)).forEach(e => g.setEdge(e.from, e.to, {}, e.id))
  dagre.layout(g)
  return new Map(graph.nodes.map(n => [n.id, { x: g.node(n.id).x - NODE_WIDTH / 2, y: g.node(n.id).y - NODE_HEIGHT / 2 }]))
}
