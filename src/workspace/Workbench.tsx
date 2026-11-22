import { useLayoutEffect, useRef, type Dispatch } from 'react'
import { scenarios } from '../demo/scenarios'
import { projectGraph, projectSession } from '../domain/project'
import { GraphCanvas } from '../graph/GraphCanvas'
import { FactDrawer, type FactFocus } from './FactDrawer'
import { InsightDrawer } from './InsightDrawer'
import { useState } from 'react'
import type { WorkspaceAction, WorkspaceState } from './state'
import type { Snapshot } from './gateway'

export function Workbench({ state, dispatch, snapshot, selectReal, onHome }: { state: WorkspaceState; dispatch: Dispatch<WorkspaceAction>; snapshot: Snapshot | null; selectReal: (session: string | null) => void; onHome?: () => void }) {
  const real = state.identity.kind === 'real'
  const taskKey = JSON.stringify(state.identity)
  const session = projectSession(state.scenario.events, state.appliedCount)
  const selectedSummary = snapshot?.sessions.find(item => item.session === snapshot.selected)
  const selectedWatermark = Math.max(selectedSummary?.last_seq ?? 0, snapshot?.watermark ?? 0)
  const syncing = state.appliedCount < selectedWatermark || ((selectedSummary?.status === 'done' || snapshot?.status === 'done') && session.status !== 'done')
  const complete = state.appliedCount >= state.scenario.events.length
  const [factFocus, setFactFocus] = useState<FactFocus | null>(null)
  const factSeq = useRef(0)
  const [insightOpenSeq, setInsightOpenSeq] = useState(0)
  const opener = useRef<HTMLElement | null>(null)
  const closeFocusSeq = useRef(0)
  const focusCleanup = useRef<(() => void) | null>(null)
  const cancelCloseFocus = () => { closeFocusSeq.current++; focusCleanup.current?.(); focusCleanup.current = null }
  useLayoutEffect(() => () => { cancelCloseFocus() }, [])
  const clearPendingFocus = () => { setFactFocus(null); opener.current = null; cancelCloseFocus() }
  const openInsight = (nodeId: string, target: HTMLElement) => { cancelCloseFocus(); opener.current = target; setInsightOpenSeq(value => value + 1); dispatch({ type: 'openInsight', nodeId }) }
  const locate = (nodeId: string) => dispatch({ type: 'locateNode', nodeId, closeOverlay: window.innerWidth <= 1180 })
  const closeInsight = () => {
    cancelCloseFocus()
    dispatch({ type: 'closeInsight' })
    const target = opener.current
    const selectedId = state.selectedId
    const request = ++closeFocusSeq.current
    const cancelOnInteraction = (event: Event) => { if (event.isTrusted) cancelCloseFocus() }
    const cancelOnFocus = () => cancelCloseFocus()
    document.addEventListener('pointerdown', cancelOnInteraction, true)
    document.addEventListener('keydown', cancelOnInteraction, true)
    document.addEventListener('focusin', cancelOnFocus, true)
    window.addEventListener('blur', cancelOnFocus)
    focusCleanup.current = () => {
      document.removeEventListener('pointerdown', cancelOnInteraction, true)
      document.removeEventListener('keydown', cancelOnInteraction, true)
      document.removeEventListener('focusin', cancelOnFocus, true)
      window.removeEventListener('blur', cancelOnFocus)
    }
    requestAnimationFrame(() => {
      if (request !== closeFocusSeq.current) return
      focusCleanup.current?.()
      focusCleanup.current = null
      if (target?.isConnected && target.getClientRects().length) target.focus()
      if (document.activeElement !== target) {
        const node = selectedId ? document.querySelector<HTMLButtonElement>(`.canvas button[data-node-id="${CSS.escape(selectedId)}"]`) : null
        if (node?.isConnected && node.getClientRects().length) node.focus()
        if (document.activeElement !== node) document.querySelector<HTMLButtonElement>('.segmented button[aria-pressed="true"]')?.focus()
      }
    })
  }
  const playbackRef = useRef<HTMLButtonElement>(null)
  const restartRef = useRef<HTMLButtonElement>(null)
  const previous = useRef({ complete, scenario: state.scenario.id })

  useLayoutEffect(() => {
    if (complete && !previous.current.complete && previous.current.scenario === state.scenario.id && document.activeElement === playbackRef.current) {
      restartRef.current?.focus()
    }
    if (playbackRef.current) playbackRef.current.hidden = complete
    previous.current = { complete, scenario: state.scenario.id }
  }, [complete, state.scenario.id])

  return (
    <main className="app">
      <header className="topbar">
        <div className="topbar-title">
          <strong className="brand">◈ MindTrail</strong>{onHome && <button onClick={onHome}>连接与任务</button>}<span className="divider" />
          <strong>{state.scenario.title}</strong><span className="badge">{real ? '真实任务' : '内置样例'}</span>
        </div>
        <div className="segmented" aria-label="视图切换">
          {(['hierarchy', 'calls'] as const).map(mode => (
            <button key={mode} aria-pressed={state.mode === mode} onClick={() => dispatch({ type: 'switchMode', mode })}>
              {mode === 'hierarchy' ? '层级视图' : '调用视图'}
            </button>
          ))}
        </div>
        <div className="toolbar">
          {real ? <span className="progress">{session.status === 'done' ? '已完成' : syncing ? '补读中' : snapshot?.error ?? `已读取 ${state.appliedCount}`}</span> : <>
          <span className="progress">{complete ? '演示完成' : `${state.appliedCount} / ${state.scenario.events.length}`}</span><span className="sr-only" aria-live="polite">{complete ? '演示完成' : ''}</span>
          <button ref={playbackRef} onClick={() => dispatch({ type: 'togglePlayback' })}>{state.playing ? '暂停' : '继续播放'}</button>
          <button ref={restartRef} onClick={() => { clearPendingFocus(); dispatch({ type: 'restart' }) }}>从头演示</button>
          </>}
        </div>
      </header>
      <div className="workspace-body">
        <aside className="task-list">
          <div className="section-label">真实任务</div>
          {snapshot?.error && <p role="status">{snapshot.error}</p>}
          {snapshot?.changedStore && <p role="status">数据库已变更</p>}
          {snapshot?.sessions.map(item => {
            const active = snapshot.selected === item.session
            const status = active ? (session.status === 'done' ? '已完成' : syncing ? '补读中' : '运行中') : item.status === 'done' ? '已完成' : '运行中'
            return <button className={`task ${active ? 'active' : ''}`} aria-current={active ? 'true' : undefined} key={item.session} onClick={() => { if (!active) { clearPendingFocus(); selectReal(item.session) } }}><span className="task-title">{item.title}</span><span className="task-status">{status} · {active ? selectedWatermark : item.last_seq}</span></button>
          })}
          <div className="section-label">内置样例 <span>{scenarios.length}</span></div>
          {scenarios.map(scenario => {
            const active = !real && scenario.id === state.scenario.id
            const status = active ? session.status : projectSession(scenario.events, scenario.initialCount).status
            return (
              <button className={`task ${active ? 'active' : ''}`} aria-current={active ? 'true' : undefined} key={scenario.id} onClick={() => { if (!active) clearPendingFocus(); selectReal(null); dispatch({ type: 'switchTask', scenario }) }}>
                <span className="task-title">{scenario.title}</span>
                <span className="task-status"><i className={status === 'done' ? 'done' : ''} />{status === 'done' ? '已完成' : '运行中'}</span>
              </button>
            )
          })}
        </aside>
        <section className="center-workspace">
          <div className="canvas" aria-label="工作区">
            <div className="canvas-heading"><span>任务地图</span><small>{state.mode === 'hierarchy' ? '层级视图' : '调用视图'} · {session.nodes.size} 个节点</small></div>
            <GraphCanvas key={taskKey} graph={projectGraph(session, state.mode)} selectedId={state.selectedId} focus={state.focus} onSelect={nodeId => dispatch({ type: 'selectNode', nodeId })} onOpenInsight={openInsight} />
          </div>
          <FactDrawer key={taskKey} session={session} source={real ? 'real' : 'demo'} selectedId={state.selectedId} expanded={state.factsExpanded} tab={state.factTab} onToggle={() => dispatch({ type: 'toggleFacts' })} onTab={tab => dispatch({ type: 'setFactTab', tab })} onOpenInsight={openInsight} focus={factFocus} onLocate={locate} />
        </section>
        {state.insightNodeId && <InsightDrawer key={taskKey + ':' + state.insightNodeId} session={session} source={real ? 'real' : 'demo'} storeId={state.identity.kind === 'real' ? state.identity.storeId : undefined} nodeId={state.insightNodeId} openSeq={insightOpenSeq} onClose={closeInsight} onLocateNode={locate} onRevealEvidence={ref => {
          if (ref.kind === 'src') return
          dispatch({ type: 'revealEvidence', nodeId: ref.nodeId, tab: ref.kind === 'quote' ? 'quotes' : 'diff', closeOverlay: window.innerWidth <= 1180 })
          setFactFocus({ requestId: ++factSeq.current, kind: ref.kind, evidenceId: ref.id })
        }} />}
      </div>
    </main>
  )
}
