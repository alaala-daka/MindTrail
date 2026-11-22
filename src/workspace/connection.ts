import { invoke } from '@tauri-apps/api/core'

export type GatewayStatus = { status: 'ready'; store_id: string } | { status: 'unavailable'; message: string }
export type ConnectionStatus = { gateway: GatewayStatus; producer_path: string | null; producer_available: boolean; data_path: string }
export type ConnectionCheck = { status: 'passed' | 'failed'; checks: { name: string; status: 'passed' | 'failed'; code: string | null }[] }
export const connectionStatus = () => invoke<ConnectionStatus>('connection_status')
export const connectionCheck = () => invoke<ConnectionCheck>('connection_check')

export function codexMcpConfig(path: string, queue: string): string {
  if (!/^(?:[A-Za-z]:\\|\\\\[^\\]+\\[^\\]+\\)/u.test(path) || /[\u0000-\u001f\u007f]/u.test(path) || [...path].some(c => { const n=c.codePointAt(0)!; return n>=0xd800 && n<=0xdfff })) throw new Error('invalid_path')
  if (!/^[a-z0-9_-]{1,64}$/.test(queue) || /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])$/.test(queue)) throw new Error('invalid_queue')
  return `[mcp_servers.mindtrail]\ncommand = ${JSON.stringify(path)}\nargs = ["mcp", "--queue", ${JSON.stringify(queue)}]\n`
}
