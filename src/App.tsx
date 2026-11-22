import { useEffect, useReducer, useRef, useState } from 'react'
import { scenarios } from './demo/scenarios'
import { createWorkspace, type WorkspaceAction } from './workspace/state'
import { appReducer } from './workspace/appState'
import { Workbench } from './workspace/Workbench'
import './workspace/workbench.css'
import { desktopClient, GatewaySync, isTauri, type GatewayClient } from './workspace/gateway'
import { ConnectionPanel } from './workspace/ConnectionPanel'
export default function App({ client = desktopClient, desktopOverride }: { client?: GatewayClient; desktopOverride?: boolean } = {}) {
  const desktop = desktopOverride ?? isTauri()
  const [welcome, setWelcome] = useState(desktop)
  const [help, setHelp] = useState(false)
  const [app, send] = useReducer(appReducer, { workspace: createWorkspace(scenarios[0]), snapshot: null })
  const { workspace: state, snapshot } = app
  const dispatch = (action: WorkspaceAction) => send({ type: 'workspace', action })
  const sync = useRef<GatewaySync | null>(null)
  useEffect(() => {
    if (!desktop) return
    const instance = new GatewaySync(client, snapshot => send({ type: 'snapshot', snapshot })); sync.current = instance
    const focus = () => instance.refresh()
    window.addEventListener('focus', focus); void instance.start()
    return () => { window.removeEventListener('focus', focus); instance.stop(); sync.current = null }
  }, [desktop, client])
  const { playing, runId, appliedCount } = state
  useEffect(() => {
    if (!playing) return
    const timer = window.setTimeout(() => dispatch({ type: 'tick', runId, expectedCount: appliedCount }), 1000)
    return () => window.clearTimeout(timer)
  }, [playing, runId, appliedCount])
  if (welcome) return <main className="app connection-home"><h1>MindTrail</h1><h2>真实任务</h2>{snapshot?.sessions.length ? snapshot.sessions.map(item => <button key={item.session} onClick={() => { sync.current?.select(item.session); setWelcome(false) }}>{item.title}</button>) : <p>暂无真实任务</p>}<nav><button onClick={() => setHelp(value => !value)}>连接帮助</button><button onClick={() => { sync.current?.select(null); dispatch({ type: 'switchTask', scenario: scenarios[0] }); setWelcome(false) }}>演示</button></nav>{help && <ConnectionPanel />}</main>
  return <Workbench state={state} dispatch={dispatch} snapshot={snapshot} selectReal={session => sync.current?.select(session)} onHome={desktop ? () => setWelcome(true) : undefined} />
}
