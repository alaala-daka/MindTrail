import { describe, expect, it } from 'vitest'
import { layoutGraph } from './layout'
import type { VisibleGraph } from '../domain/project'
const nodes = ['a', 'b', 'c'].map(id => ({ id, kind: 'file' as const, title: id, firstSeq: 1, events: [] }))
describe('layoutGraph', () => {
  it('is deterministic across ordering with parallel typed edges', () => {
    const graph: VisibleGraph = { nodes, edges: [
      { id:'one', from:'a', to:'b', kind:'calls' },
      { id:'two', from:'a', to:'b', kind:'imports' },
      { id:'zero', from:'a', to:'b', kind:'calls' },
      { id:'three', from:'b', to:'c', kind:'calls' },
    ] }
    const reversed = { nodes: [...nodes].reverse(), edges: [...graph.edges].reverse() }
    expect(Object.fromEntries(layoutGraph(graph))).toEqual(Object.fromEntries(layoutGraph(reversed)))
    expect(layoutGraph(graph).get('a')!.x).toBeLessThan(layoutGraph(graph).get('c')!.x)
  })
})
