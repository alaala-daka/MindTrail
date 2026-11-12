import { useLayoutEffect, useRef, useState } from 'react'
import { sampleInsights, type CitationRef } from '../demo/insights'
import type { SessionProjection, TrailNode } from '../domain/project'
import { getNodeEvidence, resolveCitation } from './evidence'
import './insight-drawer.css'

export type InsightDrawerProps = {
  session: SessionProjection
  nodeId: string | null
  openSeq: number
  onClose: () => void
  onLocateNode: (nodeId: string) => void
  onRevealEvidence: (ref: CitationRef) => void
}

function reachable(session: SessionProjection, from: string, target: string): boolean {
  const seen = new Set([from])
  const pending = [from]
  while (pending.length) {
    const current = pending.shift()!
    for (const edge of session.callEdges) {
      if (edge.from !== current || !session.nodes.has(edge.to) || seen.has(edge.to)) continue
      if (edge.to === target) return true
      seen.add(edge.to)
      pending.push(edge.to)
    }
  }
  return false
}

function FactList({ session, node, onLocateNode }: { session: SessionProjection; node: TrailNode; onLocateNode: (id: string) => void }) {
  const edges = session.callEdges.filter(edge => (edge.from === node.id || edge.to === node.id) && session.nodes.has(edge.from) && session.nodes.has(edge.to))
  const quotes = getNodeEvidence(session, node.id).filter(item => item.kind === 'quote')
  return <>
    <h3>事实清单</h3>
    <p>{node.kind === 'api' ? '签名' : node.kind === 'file' ? '路径' : '名称'}：{node.title}</p>
    {node.description && <p>{node.description}</p>}
    <h3>事件原话</h3>
    {quotes.length ? quotes.map(item => <p key={item.id}>{item.text}</p>) : <p>暂无原话</p>}
    <h3>上下游关系</h3>
    {edges.length ? edges.map(edge => {
      const id = edge.from === node.id ? edge.to : edge.from
      return <div key={edge.id}>{edge.to === node.id ? '上游' : '下游'} · <button onClick={() => onLocateNode(id)}>{session.nodes.get(id)!.title}</button></div>
    }) : <p>暂无调用关系</p>}
  </>
}

export function InsightDrawer({ session, nodeId, openSeq, onClose, onLocateNode, onRevealEvidence }: InsightDrawerProps) {
  const closeRef = useRef<HTMLButtonElement>(null)
  const drawerRef = useRef<HTMLElement>(null)
  const trapFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'Tab' || window.innerWidth > 1180) return
    const buttons = Array.from(drawerRef.current?.querySelectorAll<HTMLButtonElement>('button:not([disabled])') ?? []).filter(button => button.getClientRects().length > 0)
    if (!buttons.length) return
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
    if (event.shiftKey && index <= 0) { event.preventDefault(); buttons[buttons.length - 1].focus() }
    else if (!event.shiftKey && (index === -1 || index === buttons.length - 1)) { event.preventDefault(); buttons[0].focus() }
  }
  useLayoutEffect(() => { if (window.innerWidth <= 1180) closeRef.current?.focus() }, [openSeq])
  useLayoutEffect(() => {
    const keepFocus = (event: FocusEvent) => {
      if (window.innerWidth <= 1180 && !drawerRef.current?.contains(event.target as Node)) closeRef.current?.focus()
    }
    document.addEventListener('focusin', keepFocus)
    return () => document.removeEventListener('focusin', keepFocus)
  }, [])
  useLayoutEffect(() => {
    let narrow = window.innerWidth <= 1180
    const onResize = () => {
      const next = window.innerWidth <= 1180
      if (next && !narrow && !closeRef.current?.closest('aside')?.contains(document.activeElement)) closeRef.current?.focus()
      narrow = next
    }
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [])
  const [expanded, setExpanded] = useState<string | null>(null)
  const node = nodeId ? session.nodes.get(nodeId) : undefined
  const insight = node && sampleInsights.find(item => item.sessionId === session.id && item.nodeId === node.id)
  return <aside ref={drawerRef} onKeyDown={trapFocus} className="insight-drawer" aria-label="节点解读">
    <header className="insight-header">
      <div><strong>节点解读</strong><div>{node?.title ?? '暂无节点'}</div></div>
      <div>{node && <button onClick={() => onLocateNode(node.id)}>图中定位</button>} <button ref={closeRef} onClick={onClose} aria-label="关闭解读">关闭</button></div>
    </header>
    <div className="insight-body">
      {!node ? <p>节点尚不可用</p> : insight ? <>
        <p>样例解读</p>
        {insight.sections.map(section => <section key={section.title}>
          <h3>{section.title}</h3>
          <p>{section.text}</p>
          {section.citations.map(ref => {
            const item = resolveCitation(session, insight, ref)
            const label = ref.kind === 'quote' ? '原话' : ref.kind === 'diff' ? '代码变更' : '源码'
            const key = `${session.id}:${node.id}:${ref.kind}:${ref.id}`
            return <div key={key}>
              {item ? <button onClick={() => ref.kind === 'src' ? setExpanded(expanded === key ? null : key) : onRevealEvidence(ref)} aria-expanded={ref.kind === 'src' ? expanded === key : undefined}>{label}</button> : <span>{label}（暂不可用）</span>}
              {item && ref.kind === 'src' && expanded === key && <div><div>随包材料 · {item.path} · 第 {item.firstLine} 行</div><pre>{item.text}</pre></div>}
            </div>
          })}
          <div>{section.relatedNodeIds.map(id => {
            const related = session.nodes.get(id)
            return related && reachable(session, node.id, id)
              ? <button key={id} onClick={() => onLocateNode(id)}>{related.title}</button>
              : <span key={id}>{related?.title ?? id}（暂不可用）</span>
          })}</div>
        </section>)}
      </> : <FactList session={session} node={node} onLocateNode={onLocateNode} />}
    </div>
  </aside>
}
