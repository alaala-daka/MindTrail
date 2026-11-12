import { useEffect, useMemo, useRef } from 'react'
import { ReactFlow, ReactFlowProvider, useReactFlow, Handle, Position, MarkerType, type NodeProps, type Node } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import type { TrailNode, VisibleGraph } from '../domain/project'
import { layoutGraph, NODE_HEIGHT, NODE_WIDTH } from './layout'
export type GraphFocus = { requestId: number; nodeId: string; moveKeyboardFocus: boolean }
export type GraphCanvasProps = { graph: VisibleGraph; selectedId: string | null; focus: GraphFocus | null; onSelect: (nodeId: string) => void; onOpenInsight: (nodeId: string, opener: HTMLElement) => void }
type CardData = { trail: TrailNode; selected: boolean; onSelect: GraphCanvasProps['onSelect']; onOpenInsight: GraphCanvasProps['onOpenInsight'] }
type CardNode = Node<CardData, 'card'>
function Card({ data }: NodeProps<CardNode>) {
  const n = data.trail
  return <><Handle type="target" position={Position.Left} /><button data-node-id={n.id} aria-pressed={data.selected} className={`graph-card ${n.kind} ${data.selected ? 'selected' : ''}`} onClick={e => { e.stopPropagation(); if (e.detail >= 2) data.onOpenInsight(n.id, e.currentTarget); else data.onSelect(n.id) }}><span className="graph-kind">{({ task:'任务', capability:'能力', api:'接口', file:'文件', group:'分组' })[n.kind]} {n.state && <em className={n.state}>{({ planned:'计划', active:'执行中', done:'完成' })[n.state]}</em>}</span><strong title={n.title}>{n.title}</strong></button><Handle type="source" position={Position.Right} /></>
}
const nodeTypes = { card: Card }
function Canvas(props: GraphCanvasProps) {
  const { fitView, setCenter, getZoom, viewportInitialized } = useReactFlow()

  const container = useRef<HTMLDivElement>(null)
  const fitted = useRef(false)
  const done = useRef(0)
  const structure = JSON.stringify([props.graph.nodes.map(n => n.id).sort(), props.graph.edges.map(e => [e.from,e.to,e.kind,e.id]).sort((a,b) => JSON.stringify(a).localeCompare(JSON.stringify(b)))])
  const positions = useMemo(() => layoutGraph(props.graph), [structure])
  const nodes: CardNode[] = props.graph.nodes.map(n => ({ id:n.id, type:'card', initialWidth:NODE_WIDTH, initialHeight:NODE_HEIGHT, position:positions.get(n.id)!, data:{ trail:n, selected:props.selectedId === n.id, onSelect:props.onSelect, onOpenInsight:props.onOpenInsight }, draggable:false, selectable:false, focusable:false }))
  const edges = props.graph.edges.map(e => ({ id:e.id, source:e.from, target:e.to, label:e.kind === 'calls' ? '调用' : e.kind === 'imports' ? '引用' : '', markerEnd:e.kind === 'contains' ? undefined : { type:MarkerType.ArrowClosed }, style:{ stroke:'#64748b' }, labelStyle:{ fill:'#aab5c4', fontSize:11 }, labelBgStyle:{ fill:'#0d1219' } }))
  useEffect(() => { if (!fitted.current && nodes.length) { const frame = requestAnimationFrame(() => { fitted.current = true; void fitView({ padding:0.25, nodes: nodes.map(n => ({ id: n.id })) }) }); return () => cancelAnimationFrame(frame) } }, [nodes.length, fitView])
  useEffect(() => {
    if (!props.focus || props.focus.requestId === done.current) return
    const p = positions.get(props.focus.nodeId)
    if (!p) return
    let frame = 0
    let cancelled = false
    const attempt = () => {
      const button = container.current?.querySelector<HTMLButtonElement>(`[data-node-id="${CSS.escape(props.focus!.nodeId)}"]`)
      if (!button || !viewportInitialized) { frame = requestAnimationFrame(attempt); return }
      void setCenter(p.x + NODE_WIDTH / 2, p.y + NODE_HEIGHT / 2, { zoom: getZoom() }).then(success => {
        if (cancelled) return
        if (!success) { frame = requestAnimationFrame(attempt); return }
        if (props.focus!.moveKeyboardFocus) button.focus()
        done.current = props.focus!.requestId
      })
    }
    frame = requestAnimationFrame(attempt)
    return () => { cancelled = true; cancelAnimationFrame(frame) }
  }, [props.focus, positions, setCenter, getZoom, viewportInitialized])
  return <div ref={container} className="graph-container"><ReactFlow nodes={nodes} edges={edges} nodeTypes={nodeTypes} nodesDraggable={false} nodesConnectable={false} elementsSelectable={false} onNodeClick={() => {}} panOnDrag zoomOnScroll zoomOnDoubleClick={false} /><button className="fit-button" onClick={() => void fitView({ padding:0.25 })}>适应窗口</button>{nodes.length === 0 && <div className="canvas-placeholder">暂无接口或文件</div>}</div>
}
export function GraphCanvas(props: GraphCanvasProps): React.JSX.Element { return <ReactFlowProvider><Canvas {...props}/></ReactFlowProvider> }
