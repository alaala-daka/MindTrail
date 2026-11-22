import { StrictMode, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { scenarios } from '../src/demo/scenarios'
import { projectSession } from '../src/domain/project'
import { InsightDrawer } from '../src/workspace/InsightDrawer'
import type { SemanticClient, SemanticContext, SemanticPreview, SemanticResult } from '../src/workspace/semantic'

let prepared: SemanticContext | undefined
let finish: ((result: SemanticResult) => void) | undefined
let failures = ''
let configured = true
let generateCount = 0, cancelCount = 0
let nextTicket = 0, inflight: string | null = null
const tickets = new Map<string, SemanticContext>()
let finishPrepare: (() => void) | undefined
const client: SemanticClient = {
  status: async () => ({ configured, provider: configured ? 'mock' : null, model: configured ? 'm' : null, endpoint_origin: configured ? 'https://example.invalid' : null }),
  prepare: async context => { if (tickets.size >= 8) throw 'busy'; prepared = context; const id = `ticket-${++nextTicket}`; tickets.set(id,context); const value: SemanticPreview = { ticket_id:id, context: failures === 'wrong-context' ? { ...context, store_id:'other' } : context, provider:'mock', model:'m', prompt_version:'1', event_count:1, evidence_count:1, byte_count:20, events:[{ text:'待发送的合成事实' }], evidence:[{id:'auth:3',node_id:'login-api',seq:3,text:'合成证据'}], expires_at: failures === 'expired' ? 1 : Math.floor(Date.now()/1000)+300 }; if (failures === 'defer-prepare') return new Promise(resolve => { finishPrepare = () => resolve(value) }); return value },
  generate: async id => { generateCount++; tickets.delete(id); inflight=id; if (failures && failures !== 'expired') throw failures; return new Promise(resolve => { finish = resolve }) },
  cancel: async () => { cancelCount++ },
  release: async (id, context) => { if (JSON.stringify(tickets.get(id)) === JSON.stringify(context)) tickets.delete(id); if (inflight === id) { cancelCount++; inflight=null } },
}
function Harness() {
  const [prefix,setPrefix]=useState(11); const [node,setNode]=useState('login-api'); const [store,setStore]=useState('a'); const [opened,setOpened]=useState(true); const [out,setOut]=useState('')
  const resolve = () => { if (finish && prepared) { inflight=null; finish({ context: prepared, provider:'mock', model:'m', prompt_version:'1', sections:[{title:'合成标题',text:'合成结果<script>window.__injected=1</script>',citations:[{kind:'quote',session_id:'auth',node_id:'login-api',id:'auth:3'}],related_node_ids:[]}] }) } }
  return <><button onClick={() => setPrefix(7)}>切换前缀</button><button onClick={() => setNode('handler')}>切换节点</button><button onClick={() => setStore('b')}>切换库</button><button onClick={() => { configured=false; setStore('unconfigured') }}>未配置</button><button onClick={() => setOpened(false)}>卸载抽屉</button><button onClick={resolve}>完成模拟</button><button onClick={() => finishPrepare?.()}>完成准备</button><button onClick={() => setOut(`${generateCount}:${cancelCount}`)}>读取计数</button><button onClick={() => setOut(`tickets:${tickets.size}`)}>读取票据</button>{['expired','unauthorized','rate_limited','timeout','transport','defer-prepare','wrong-context'].map(x => <button key={x} onClick={() => { failures=x }}>故障 {x}</button>)}<output>{out}</output>{opened && <InsightDrawer source="real" storeId={store} semanticClient={client} session={projectSession(scenarios[0].events,prefix)} nodeId={node} openSeq={1} onClose={() => setOpened(false)} onLocateNode={() => {}} onRevealEvidence={ref => setOut(`reveal:${ref.id}`)} />}</>
}
createRoot(document.getElementById('root')!).render(<StrictMode><Harness /></StrictMode>)
