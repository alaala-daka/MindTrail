import { useEffect, useLayoutEffect, useReducer, useRef, useState } from 'react'
import { sampleInsights } from '../demo/insights'
import type { CitationRef, DataSource } from '../domain/insights'
import type { SessionProjection, TrailNode } from '../domain/project'
import { getNodeEvidence, resolveCitation } from './evidence'
import { semanticClient as defaultClient, type SemanticClient, type SemanticContext, type ProviderStatus } from './semantic'
import { contextKey, semanticReducer } from './semanticState'
import './insight-drawer.css'

export type InsightDrawerProps = {
  session: SessionProjection
  source: DataSource
  storeId?: string
  semanticClient?: SemanticClient
  nodeId: string | null
  openSeq: number
  onClose: () => void
  onLocateNode: (nodeId: string) => void
  onRevealEvidence: (ref: CitationRef) => void
}

const errors: Record<string, string> = { not_configured: '尚未配置 AI 服务。', invalid_config: 'AI 服务配置无效。', gateway_unavailable: '本地任务数据暂不可用。', identity_changed: '任务库已变化，请重新准备。', invalid_context: '预览已过期，请重新准备。', input_too_large: '所选事实范围过大。', insufficient_evidence: '当前范围没有足够的原话证据。', busy: '请求过多，请稍后重试。', cancelled: '生成已取消。', timeout: '请求超时，请重试。', transport: '无法连接 AI 服务，请重试。', unauthorized: '服务未授权，请检查配置。', rate_limited: '服务请求受限，请稍后重试。', provider_unavailable: 'AI 服务暂不可用，请重试。', invalid_output: '服务返回格式不兼容，请更换兼容模型后重试。', invalid_reference: '解读引用无效，请重新生成。' }
function errorMessage(error: unknown): string { return typeof error === 'string' && errors[error] ? errors[error] : '解读暂不可用，请重试。' }

function RealInsight({ session, node, storeId, client, onRevealEvidence }: { session: SessionProjection; node: TrailNode; storeId: string; client: SemanticClient; onRevealEvidence: (ref: CitationRef) => void }) {
  const context: SemanticContext = { store_id: storeId, session_id: session.id, node_id: node.id, prefix_seq: session.lastSeq }
  const key = contextKey(context)
  const [state, dispatch] = useReducer(semanticReducer, { status: 'idle', contextKey: key, requestId: '', generation: 0 })
  const [status, setStatus] = useState<ProviderStatus | null>(null)
  const [statusError, setStatusError] = useState<string | null>(null)
  const generation = useRef(0)
  const active = useRef<string | null>(null)
  const ticket = useRef<{ id: string; context: SemanticContext } | null>(null)
  useEffect(() => {
    let live = true
    generation.current++
    dispatch({ type: 'invalidate', contextKey: key, generation: generation.current })
    void client.status().then(value => { if (live) setStatus(value) }, error => { if (live) setStatusError(errorMessage(error)) })
    return () => { live = false; generation.current++; const owned = ticket.current; ticket.current = null; if (owned) void client.release(owned.id, owned.context).catch(() => {}) }
  }, [client, key])
  const cancel = () => { const owned = ticket.current; ticket.current = null; active.current = null; generation.current++; dispatch({ type: 'invalidate', contextKey: key, generation: generation.current }); if (owned) void client.release(owned.id, owned.context).catch(() => {}) }
  const prepare = async () => {
    if (active.current) return
    const id = crypto.randomUUID(), gen = generation.current
    active.current = id
    dispatch({ type: 'start', status: 'preparing', contextKey: key, requestId: id, generation: gen })
    try {
      const old = ticket.current
      if (old) { await client.release(old.id, old.context); if (ticket.current === old) ticket.current = null }
      if (generation.current !== gen) return
      const preview = await client.prepare(context)
      if (generation.current !== gen || contextKey(preview.context) !== key) {
        await client.release(preview.ticket_id, context)
        if (generation.current === gen) dispatch({ type: 'failed', contextKey: key, requestId: id, generation: gen, error: errors.invalid_context })
        return
      }
      ticket.current = { id: preview.ticket_id, context: preview.context }
      dispatch({ type: 'prepared', contextKey: key, requestId: id, generation: gen, preview })
    }
    catch (error) { dispatch({ type: 'failed', contextKey: key, requestId: id, generation: gen, error: errorMessage(error) }) }
    finally { if (active.current === id) active.current = null }
  }
  const generate = async () => {
    if (!state.preview || active.current) return
    if (state.preview.expires_at * 1000 <= Date.now()) { const old=ticket.current; ticket.current=null; if (old) void client.release(old.id,old.context).catch(() => {}); dispatch({ type: 'failed', contextKey: key, requestId: state.requestId, generation: state.generation, error: errors.invalid_context }); return }
    const id = crypto.randomUUID(), gen = generation.current; active.current = id
    dispatch({ type: 'start', status: 'generating', contextKey: key, requestId: id, generation: gen })
    try { const result = await client.generate(state.preview.ticket_id, id); dispatch({ type: 'resolved', contextKey: key, requestId: id, generation: gen, result }) }
    catch (error) { dispatch({ type: 'failed', contextKey: key, requestId: id, generation: gen, error: errorMessage(error) }) }
    finally { if (active.current === id) { active.current = null; const owned=ticket.current; ticket.current = null; if (owned) void client.release(owned.id,owned.context).catch(() => {}) } }
  }
  const insight = state.result && { sessionId: state.result.context.session_id, nodeId: state.result.context.node_id, sections: state.result.sections.map(s => ({ title: s.title, text: s.text, citations: s.citations.map(c => ({ kind: c.kind, id: c.id, nodeId: c.node_id, sessionId: c.session_id })), relatedNodeIds: s.related_node_ids })) }
  return <section aria-label="AI 解读">
    <h3>AI 解读</h3>
    {statusError && <><p role="alert">{statusError}</p><button onClick={() => { setStatusError(null); void client.status().then(setStatus, error => setStatusError(errorMessage(error))) }}>重新检查服务</button></>}
    {status && !status.configured && <p>尚未配置 AI 服务。</p>}
    {state.status === 'error' && <p role="alert">{state.error}</p>}
    {state.status === 'preparing' && <p>正在准备发送范围…</p>}
    {state.status === 'generating' && <><p>正在生成…</p><button onClick={cancel}>取消生成</button></>}
    {status?.configured && !['preparing', 'generating'].includes(state.status) && <button onClick={prepare}>准备解读</button>}
    {state.status === 'preview' && state.preview && <div><h4>发送预览</h4><p>服务：{state.preview.provider} / {state.preview.model} · {status?.endpoint_origin}</p><p>任务：{state.preview.context.session_id} · 节点：{state.preview.context.node_id} · 前缀：{state.preview.context.prefix_seq}</p><p>{state.preview.event_count} 条事件 · {state.preview.evidence_count} 条原话 · {state.preview.byte_count} 字节</p><pre>{JSON.stringify({ events: state.preview.events, evidence: state.preview.evidence }, null, 2)}</pre><button onClick={generate}>发送并生成</button><button onClick={cancel}>取消</button></div>}
    {state.status === 'ready' && insight && state.result && <div><p>AI 解读 · {state.result.provider}/{state.result.model} · 前缀 {state.result.context.prefix_seq}</p>{insight.sections.map((section, i) => <section key={i}><h4>{section.title}</h4><p>{section.text}</p>{section.citations.map((ref, j) => <span key={j}>{ref.sessionId === session.id && resolveCitation(session, insight, ref, 'real') ? <button onClick={() => onRevealEvidence(ref)}>原话</button> : '原话（暂不可用）'}</span>)}</section>)}</div>}
  </section>
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

function FactList({ session, node, source, onLocateNode }: { session: SessionProjection; node: TrailNode; source: DataSource; onLocateNode: (id: string) => void }) {
  const edges = session.callEdges.filter(edge => (edge.from === node.id || edge.to === node.id) && session.nodes.has(edge.from) && session.nodes.has(edge.to))
  const quotes = getNodeEvidence(session, node.id, source).filter(item => item.kind === 'quote')
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

export function InsightDrawer({ session, source, storeId, semanticClient = defaultClient, nodeId, openSeq, onClose, onLocateNode, onRevealEvidence }: InsightDrawerProps) {
  const closeRef = useRef<HTMLButtonElement>(null)
  const drawerRef = useRef<HTMLElement>(null)
  const trapFocus = (event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape') { event.stopPropagation(); onClose(); return }
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
  const insight = source === 'demo' && node ? sampleInsights.find(item => item.sessionId === session.id && item.nodeId === node.id) : undefined
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
            const item = resolveCitation(session, insight, ref, source)
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
      </> : <><FactList session={session} node={node} source={source} onLocateNode={onLocateNode} />{source === 'real' && storeId && <RealInsight key={JSON.stringify([storeId,session.id,node.id,session.lastSeq])} session={session} node={node} storeId={storeId} client={semanticClient} onRevealEvidence={onRevealEvidence} />}</>}
    </div>
  </aside>
}
