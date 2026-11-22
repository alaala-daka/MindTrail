import { useLayoutEffect, useRef } from 'react'
import type { SessionProjection } from '../domain/project'
import { getNodeEvidence } from './evidence'
import type { FactTab } from './state'
import type { DataSource } from '../domain/insights'

export type FactFocus = { requestId: number; kind: 'quote' | 'diff'; evidenceId: string }

export function FactDrawer({ session, source, selectedId, expanded, tab, focus, onToggle, onTab, onLocate, onOpenInsight }: {
  session: SessionProjection; source: DataSource; selectedId: string | null; expanded: boolean; tab: FactTab; focus?: FactFocus | null
  onToggle: () => void; onTab: (tab: FactTab) => void; onLocate: (nodeId: string) => void; onOpenInsight: (nodeId: string, opener: HTMLElement) => void
}) {
  const content = useRef<HTMLDivElement>(null)
  const consumed = useRef<{ sessionId: string; requestId: number } | null>(null)
  const node = selectedId ? session.nodes.get(selectedId) : undefined
  const evidence = node ? getNodeEvidence(session, node.id, source) : []
  useLayoutEffect(() => {
    if (!focus || !expanded || (consumed.current?.sessionId === session.id && consumed.current.requestId === focus.requestId)) return
    const target = Array.from(content.current?.querySelectorAll<HTMLElement>('[data-evidence-kind]') ?? []).find(item => item.dataset.evidenceKind === focus.kind && item.dataset.evidenceId === focus.evidenceId)
    if (!target || target.getClientRects().length === 0) return
    target.scrollIntoView({ block: 'nearest' })
    target.focus()
    if (document.activeElement === target) consumed.current = { sessionId: session.id, requestId: focus.requestId }
  })
  const items = evidence.filter(item => item.kind === (tab === 'quotes' ? 'quote' : 'diff'))
  const edges = node ? session.callEdges.filter(edge => edge.from === node.id || edge.to === node.id) : []
  return <section className={`facts ${expanded ? '' : 'collapsed'}`}>
    <div className="facts-header"><strong>事实详情{node ? ` · ${node.title}` : ''}</strong><span>{node && <button onClick={event => onOpenInsight(node.id, event.currentTarget)}>打开解读</button>} <button onClick={onToggle}>{expanded ? '收起' : '展开'}</button></span></div>
    {expanded && <><div className="fact-tabs">{([['quotes', '原文'], ['diff', '代码变更'], ['calls', '调用关系'], ['meta', '元信息']] as const).map(([key, label]) => <button key={key} aria-pressed={tab === key} onClick={() => onTab(key)}>{label}</button>)}</div>
      <div className="facts-content" ref={content}>{!node ? '选择节点查看证据' : (tab === 'quotes' || tab === 'diff') ? items.length ? items.map(item => <article key={`${item.kind}:${item.id}`} data-evidence-kind={item.kind} data-evidence-id={item.id} tabIndex={-1}><div>{item.kind === 'quote' ? `${item.id} · ${item.ts ?? ''}` : `${item.path} · 随包材料`}</div>{item.context && <div>上下文：{item.context}</div>}<pre>{item.text}</pre></article>) : '暂无相关资料' : tab === 'calls' ? edges.length ? edges.map(edge => { const other = edge.from === node.id ? edge.to : edge.from; return <div key={edge.id}>{edge.to === node.id ? '上游' : '下游'} · {edge.kind === 'calls' ? '调用' : '引用'} · <button onClick={() => onLocate(other)}>{session.nodes.get(other)?.title ?? other}</button></div> }) : '暂无调用关系' : <div><div>类型：{node.kind}</div><div>名称：{node.title}</div>{node.state && <div>能力状态：{node.state}</div>}<div>首次出现：序号 {node.firstSeq} · {node.events[0]?.ts}</div>{node.litAt && <div>点亮时间：{node.litAt}</div>}{node.events.map(event => event.type === 'file.change' ? <div key={event.seq}>变更：{event.payload.action} · +{event.payload.stats.add} / -{event.payload.stats.del}</div> : event.type === 'api.expose' ? <div key={event.seq}>接口类型：{event.payload.kind} · {event.payload.signature}</div> : null)}</div>}</div></>}
  </section>
}
