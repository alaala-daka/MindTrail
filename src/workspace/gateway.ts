import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { TrailEvent } from '../domain/events'

export type Summary = { session: string; title: string; project: string | null; status: string; last_seq: number }
export type GatewayStatus = { status: 'ready'; store_id: string } | { status: 'unavailable'; message: string }
export type GatewayClient = {
  status(): Promise<GatewayStatus>
  sessions(after: string | null, limit: number): Promise<{ store_id: string; sessions: Summary[] }>
  events(session: string, since: number, limit: number): Promise<{ store_id: string; page: { events: TrailEvent[]; last_seq: number; status: string } }>
  subscribe(callback: (session: string) => void): Promise<() => void>
}
export const desktopClient: GatewayClient = {
  status: () => invoke('gateway_status'),
  sessions: (after, limit) => invoke('list_sessions', { after, limit }),
  events: (session, since, limit) => invoke('read_events', { session, since, limit }),
  subscribe: callback => listen<string>('mindtrail:session-updated', event => callback(event.payload)),
}
export { isTauri }
export type Snapshot = { storeId: string | null; sessions: Summary[]; selected: string | null; events: TrailEvent[]; watermark: number; status: string | null; error: string | null; changedStore: boolean }
function binaryCompare(a: string, b: string): number {
  const encoder = new TextEncoder(), left = encoder.encode(a), right = encoder.encode(b)
  for (let i = 0; i < Math.min(left.length, right.length); i++) if (left[i] !== right[i]) return left[i] - right[i]
  return left.length - right.length
}
export class GatewaySync {
  private state: Snapshot = { storeId: null, sessions: [], selected: null, events: [], watermark: 0, status: null, error: null, changedStore: false }
  private generation = 0
  private running = { status: false, list: false, events: false }
  private pending = { status: false, list: false, events: false }
  private errors = { status: null as string | null, list: null as string | null, events: null as string | null }
  private storeGeneration = 0
  private stopped = false
  private timer?: ReturnType<typeof setInterval>
  private unlisten?: () => void
  constructor(private client: GatewayClient, private publish: (snapshot: Snapshot) => void) {}
  private emit() { this.publish({ ...this.state }) }
  async start() {
    this.stopped = false
    this.timer = setInterval(() => this.refresh(), 1000)
    this.refresh()
    try { const unlisten = await this.client.subscribe(() => this.refresh()); if (this.stopped) unlisten(); else this.unlisten = unlisten } catch { /* polling remains available */ }
  }
  stop() { this.stopped = true; this.generation++; clearInterval(this.timer); this.unlisten?.() }
  select(session: string | null) {
    if (session === this.state.selected) return
    this.generation++; this.errors.events = null
    const summary = this.state.sessions.find(item => item.session === session)
    this.state = { ...this.state, selected: session, events: [], watermark: summary?.last_seq ?? 0, status: summary?.status ?? null, error: this.errors.status ?? this.errors.list }
    this.emit(); this.refresh()
  }
  refresh() { this.schedule('status') }
  private schedule(kind: 'status' | 'list' | 'events') {
    if (this.stopped) return
    if (this.running[kind]) { this.pending[kind] = true; return }
    void this.run(kind)
  }
  private async run(kind: 'status' | 'list' | 'events') {
    this.running[kind] = true
    const generation = this.generation, storeGeneration = this.storeGeneration
    const valid = () => !this.stopped && (kind === 'status' || (storeGeneration === this.storeGeneration && (kind !== 'events' || generation === this.generation)))
    try {
      if (kind === 'status') {
        const status = await this.client.status(); if (!valid()) return
        if (status.status !== 'ready') throw new Error(status.message)
        if (this.state.storeId !== status.store_id) {
          const changedStore = this.state.storeId !== null
          this.storeGeneration++
          this.errors.list = null; this.errors.events = null; this.errors.status = null
          this.state = { ...this.state, storeId: status.store_id, sessions: [], events: [], watermark: 0, status: null, error: null, changedStore }
          this.emit()
        }
        this.schedule('list'); this.schedule('events')
      } else if (kind === 'list') {
      const store = this.state.storeId
      let after: string | null = null
      const sessions: Summary[] = []
      while (true) {
        const result = await this.client.sessions(after, 200); if (!valid()) return
        if (result.store_id !== store) throw new Error('数据库身份发生变化，请重试')
        for (const item of result.sessions) { if (after !== null && binaryCompare(item.session, after) <= 0) throw new Error('任务列表顺序无效'); after = item.session; sessions.push(item) }
        if (result.sessions.length < 200) break
      }
      const summary = sessions.find(item => item.session === this.state.selected)
      this.state = { ...this.state, sessions, watermark: Math.max(this.state.watermark, summary?.last_seq ?? 0), status: this.state.status === 'done' || summary?.status === 'done' ? 'done' : this.state.status ?? summary?.status ?? null }; this.emit()
      } else {
      const store = this.state.storeId
      const selected = this.state.selected
      if (selected) {
        let cursor = this.state.events[this.state.events.length - 1]?.seq ?? 0
        while (true) {
          const result = await this.client.events(selected, cursor, 200); if (!valid()) return
          if (result.store_id !== store) throw new Error('数据库身份发生变化，请重试')
          const { events, last_seq, status: pageStatus } = result.page
          let next = cursor
          for (const event of events) { if (event.session !== selected || event.seq !== ++next) throw new Error('事件序列不连续') }
          if (last_seq < next || (!events.length && cursor < last_seq)) throw new Error('事件页水位无效')
          this.state = { ...this.state, events: [...this.state.events, ...events], watermark: Math.max(this.state.watermark, last_seq), status: this.state.status === 'done' || pageStatus === 'done' ? 'done' : pageStatus }; this.emit()
          cursor = next
          if (cursor >= last_seq) break
        }
      }
      }
      if (valid()) this.errors[kind] = null
    } catch (error) { if (valid()) this.errors[kind] = String(error) }
    finally {
      if (valid()) { this.state = { ...this.state, error: this.errors.status ?? this.errors.list ?? this.errors.events }; this.emit() }
      this.running[kind] = false
      if (this.pending[kind] || !valid()) { this.pending[kind] = false; if (!this.stopped) this.schedule(kind) }
    }
  }
}



