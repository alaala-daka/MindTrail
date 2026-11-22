import { useEffect, useReducer, useRef } from 'react'
import { createRoot } from 'react-dom/client'
import { scenarios } from '../src/demo/scenarios'
import { appReducer } from '../src/workspace/appState'
import { createWorkspace, type WorkspaceAction } from '../src/workspace/state'
import { GatewaySync, type GatewayClient } from '../src/workspace/gateway'
import { Workbench } from '../src/workspace/Workbench'
import '../src/workspace/workbench.css'
const source = scenarios[0].events.map(event => event.type === 'api.expose' && event.payload.id === 'login-api' ? { ...event, payload: { ...event.payload, quote: '真实任务独有证据' } } : event)
const client: GatewayClient = {
  status: async () => ({ status: 'ready', store_id: 'isolated' }),
  sessions: async () => ({ store_id: 'isolated', sessions: [{ session: 'auth', title: '真实认证任务', project: 'test', status: 'done', last_seq: source.length }] }),
  events: async (_session, since) => ({ store_id: 'isolated', page: { events: source.slice(since), status: 'done', last_seq: source.length } }),
  subscribe: async () => () => {},
}
function Harness() {
  const [app, send] = useReducer(appReducer, { workspace: createWorkspace(scenarios[0]), snapshot: null })
  const ref = useRef<GatewaySync | null>(null)
  useEffect(() => { const sync = new GatewaySync(client, snapshot => send({ type: 'snapshot', snapshot })); ref.current = sync; void sync.start(); const focus = () => sync.refresh(); window.addEventListener('focus', focus); return () => { window.removeEventListener('focus', focus); sync.stop() } }, [])
  return <Workbench state={app.workspace} snapshot={app.snapshot} dispatch={(action: WorkspaceAction) => send({ type: 'workspace', action })} selectReal={session => ref.current?.select(session)} />
}
createRoot(document.getElementById('root')!).render(<Harness />)
