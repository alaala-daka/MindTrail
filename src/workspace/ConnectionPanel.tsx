import { useEffect, useState } from 'react'
import { codexMcpConfig, connectionCheck, connectionStatus, type ConnectionCheck, type ConnectionStatus } from './connection'

type ConnectionClient = { status: typeof connectionStatus; check: typeof connectionCheck }
const defaultClient: ConnectionClient = { status: connectionStatus, check: connectionCheck }
export function ConnectionPanel({ client = defaultClient }: { client?: ConnectionClient }) {
  const [status,setStatus]=useState<ConnectionStatus | null>(null)
  const [check,setCheck]=useState<ConnectionCheck | null>(null)
  const [message,setMessage]=useState('')
  const [busy,setBusy]=useState(false)
  useEffect(() => { let live=true; void client.status().then(value => { if(live) setStatus(value) }, () => { if(live) setMessage('无法读取本机连接状态。') }); return () => { live=false } },[client])
  let config: string | null = null
  try { if (status?.producer_available && status.producer_path) config=codexMcpConfig(status.producer_path,'codex') } catch { config=null }
  const copy=async () => { if(!config) return; try { await navigator.clipboard.writeText(config); setMessage('配置已复制。') } catch { setMessage('复制失败，请手动选择配置文本。') } }
  const runCheck=async () => { setBusy(true); setMessage(''); try { setCheck(await client.check()) } catch { setCheck(null); setMessage('本机检查无法完成。') } finally { setBusy(false) } }
  return <section aria-label="连接帮助">
    <h2>连接 Agent</h2><p>网关：{!status ? '检查中' : status.gateway.status === 'ready' ? '可用' : '不可用'}</p>
    {status && <p>数据位置：{status.data_path}</p>}
    {!status ? <p>正在检查连接程序…</p> : config ? <><p>程序：{status.producer_path}</p><p>将以下配置复制到自己的 Codex MCP 配置中，然后重启客户端：</p><pre>{config}</pre><button onClick={copy}>复制配置</button></> : <p>连接程序尚未随此版本安装或路径不可用。</p>}
    <p>按任务实际进度使用 trace_task_start、trace_intent、trace_api、trace_file、trace_call、trace_done 和 trace_task_done 上报。</p>
    <p>queued 表示已排队，committed 表示已提交；任务结束仍需明确上报。上报内容由 Agent 提供。</p>
    <button onClick={runCheck} disabled={busy}>本机自检</button>
    {check && <><p role="status">{check.status === 'passed' ? '本机检查通过。' : '本机检查未通过。'}仅检查本机状态，尚未证明 Agent 或 AI 已接入。</p><ul>{check.checks.map(item => <li key={item.name}>{item.name === 'producer' ? '连接程序' : '本地网关'}：{item.status === 'passed' ? '可用' : '不可用'}</li>)}</ul></>}
    {message && <p role="status">{message}</p>}
  </section>
}
