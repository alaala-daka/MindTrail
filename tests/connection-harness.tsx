import { createRoot } from 'react-dom/client'
import { ConnectionPanel } from '../src/workspace/ConnectionPanel'

let copied = ''
let copyFails = false
Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (value: string) => { if (copyFails) throw new Error('synthetic'); copied=value } }, configurable: true })
const missing = new URLSearchParams(location.search).has('missing')
const client = {
  status: async () => ({ gateway: { status:'ready' as const, store_id:'synthetic' }, producer_path:missing ? null : 'C:\\Program Files\\MindTrail\\producer\\mindtrail-producer.exe', producer_available:!missing, data_path:'C:\\Synthetic\\.path-tracker' }),
  check: async () => ({ status:missing ? 'failed' as const : 'passed' as const, checks:[] }),
}
createRoot(document.getElementById('root')!).render(<><ConnectionPanel client={client} /><button onClick={() => { document.querySelector('output')!.textContent=copied }}>检查剪贴板</button><button onClick={() => { copyFails=true }}>模拟复制失败</button><output /></>)
