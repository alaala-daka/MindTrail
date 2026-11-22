import { createRoot } from 'react-dom/client'
import App from '../src/App'
import { scenarios } from '../src/demo/scenarios'
import type { GatewayClient } from '../src/workspace/gateway'

const events = scenarios[0].events.map(event => ({ ...event, session: 'real-task' }))
const client: GatewayClient = {
  status: async () => ({ status:'ready', store_id:'synthetic-store' }),
  sessions: async after => ({ store_id:'synthetic-store', sessions:after ? [] : [{session:'real-task',title:'真实合成任务',project:'synthetic',status:'done',last_seq:events.length}] }),
  events: async (_session,since,limit) => ({ store_id:'synthetic-store', page:{events:events.slice(since,since+limit),last_seq:events.length,status:'done'} }),
  subscribe: async () => () => {},
}
createRoot(document.getElementById('root')!).render(<App desktopOverride client={client} />)
