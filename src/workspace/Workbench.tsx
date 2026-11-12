import { useLayoutEffect, useRef, type Dispatch } from 'react'
import { scenarios } from '../demo/scenarios'
import { projectGraph, projectSession } from '../domain/project'
import { GraphCanvas } from '../graph/GraphCanvas'
import { FactDrawer, type FactFocus } from './FactDrawer'
import { InsightDrawer } from './InsightDrawer'
import { useState } from 'react'
import type { WorkspaceAction, WorkspaceState } from './state'

export function Workbench({ state, dispatch }: { state: WorkspaceState; dispatch: Dispatch<WorkspaceAction> }) {
  const session = projectSession(state.scenario.events, state.appliedCount)
  const complete = state.appliedCount >= state.scenario.events.length
  const [factFocus, setFactFocus] = useState<FactFocus | null>(null)
  const factSeq = useRef(0)
  const [insightOpenSeq, setInsightOpenSeq] = useState(0)
  const opener = useRef<HTMLElement | null>(null)
  const closeFocusSeq = useRef(0)
  const clearPendingFocus = () => { setFactFocus(null); opener.current = null; closeFocusSeq.current++ }
  const openInsight = (nodeId: string, target: HTMLElement) => { closeFocusSeq.current++; opener.current = target; setInsightOpenSeq(value => value + 1); dispatch({ type: 'openInsight', nodeId }) }
  const locate = (nodeId: string) => dispatch({ type: 'locateNode', nodeId, closeOverlay: window.innerWidth <= 1180 })
  const closeInsight = () => { dispatch({ type: 'closeInsight' }); const target = opener.current; const selectedId = state.selectedId; const request = ++closeFocusSeq.current; requestAnimationFrame(() => { if (request !== closeFocusSeq.current) return; if (target?.isConnected && target.getClientRects().length) target.focus(); if (document.activeElement !== target) { if (selectedId) dispatch({ type: 'focusNode', nodeId: selectedId, moveKeyboardFocus: true }); else document.querySelector<HTMLButtonElement>('.segmented button[aria-pressed="true"]')?.focus() } }) }
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
          <strong className="brand">◈ MindTrail</strong><span className="divider" />
          <strong>{state.scenario.title}</strong><span className="badge">内置样例</span>
        </div>
        <div className="segmented" aria-label="视图切换">
          {(['hierarchy', 'calls'] as const).map(mode => (
            <button key={mode} aria-pressed={state.mode === mode} onClick={() => dispatch({ type: 'switchMode', mode })}>
              {mode === 'hierarchy' ? '层级视图' : '调用视图'}
            </button>
          ))}
        </div>
        <div className="toolbar">
          <span className="progress">{complete ? '演示完成' : `${state.appliedCount} / ${state.scenario.events.length}`}</span><span className="sr-only" aria-live="polite">{complete ? '演示完成' : ''}</span>
          <button ref={playbackRef} onClick={() => dispatch({ type: 'togglePlayback' })}>{state.playing ? '暂停' : '继续播放'}</button>
          <button ref={restartRef} onClick={() => { clearPendingFocus(); dispatch({ type: 'restart' }) }}>从头演示</button>
        </div>
      </header>
      <div className="workspace-body">
        <aside className="task-list">
          <div className="section-label">任务列表 <span>{scenarios.length}</span></div>
          {scenarios.map(scenario => {
            const active = scenario.id === state.scenario.id
            const status = active ? session.status : projectSession(scenario.events, scenario.initialCount).status
            return (
              <button className={`task ${active ? 'active' : ''}`} aria-current={active ? 'true' : undefined} key={scenario.id} onClick={() => { if (!active) clearPendingFocus(); dispatch({ type: 'switchTask', scenario }) }}>
                <span className="task-title">{scenario.title}</span>
                <span className="task-status"><i className={status === 'done' ? 'done' : ''} />{status === 'done' ? '已完成' : '运行中'}</span>
              </button>
            )
          })}
        </aside>
        <section className="center-workspace">
          <div className="canvas" aria-label="工作区">
            <div className="canvas-heading"><span>任务地图</span><small>{state.mode === 'hierarchy' ? '层级视图' : '调用视图'} · {session.nodes.size} 个节点</small></div>
            <GraphCanvas key={state.scenario.id} graph={projectGraph(session, state.mode)} selectedId={state.selectedId} focus={state.focus} onSelect={nodeId => dispatch({ type: 'selectNode', nodeId })} onOpenInsight={openInsight} />
          </div>
          <FactDrawer key={state.scenario.id} session={session} selectedId={state.selectedId} expanded={state.factsExpanded} tab={state.factTab} onToggle={() => dispatch({ type: 'toggleFacts' })} onTab={tab => dispatch({ type: 'setFactTab', tab })} onOpenInsight={openInsight} focus={factFocus} onLocate={locate} />
        </section>
        {state.insightNodeId && <InsightDrawer key={state.scenario.id + ':' + state.insightNodeId} session={session} nodeId={state.insightNodeId} openSeq={insightOpenSeq} onClose={closeInsight} onLocateNode={locate} onRevealEvidence={ref => {
          if (ref.kind === 'src') return
          dispatch({ type: 'revealEvidence', nodeId: ref.nodeId, tab: ref.kind === 'quote' ? 'quotes' : 'diff', closeOverlay: window.innerWidth <= 1180 })
          setFactFocus({ requestId: ++factSeq.current, kind: ref.kind, evidenceId: ref.id })
        }} />}
      </div>
    </main>
  )
}
