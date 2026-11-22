import { describe, it, expect, vi } from 'vitest'
import { GatewaySync, type GatewayClient, type Snapshot } from './gateway'
import type { TrailEvent } from '../domain/events'
import { scenarios } from '../demo/scenarios'
import { projectGraph, projectSession } from '../domain/project'
import { getNodeEvidence } from './evidence'
const event = (seq: number): TrailEvent => ({ v: 1, session: 's', seq, ts: '2026-01-01T00:00:00Z', type: 'task.start', payload: { title: 'x', goal: 'x', project: 'x' } })
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve() }
describe('GatewaySync', () => {
  it('retains the highest observed watermark through errors and clears it for another identity', async () => {
    let store = 'one', advanced = false, fail = false
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: store }),
      sessions: async () => ({ store_id: store, sessions: store === 'one' ? [{ session: 's', title: 's', project: '', status: advanced ? 'done' : 'open', last_seq: advanced ? 2 : 1 }] : [] }),
      events: async (session, since) => {
        if (fail) throw new Error('read failed')
        return { store_id: store, page: { events: session === 's' && since === 0 ? [event(1)] : session === 's' && since === 1 && advanced ? [event(2)] : [], last_seq: session === 's' ? advanced ? 2 : 1 : 0, status: 'open' } }
      },
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); sync.select('s'); await flush()
    advanced = true; fail = true; sync.refresh(); await flush()
    expect(snapshot).toMatchObject({ watermark: 2, status: 'done' }); expect(snapshot.events).toHaveLength(1); expect(snapshot.error).toContain('read failed')
    advanced = false; fail = false; sync.refresh(); await flush()
    expect(snapshot).toMatchObject({ watermark: 2, status: 'done', error: null })
    advanced = true; sync.refresh(); await flush(); expect(snapshot.events).toHaveLength(2)
    sync.select('other'); expect(snapshot).toMatchObject({ selected: 'other', watermark: 0, status: null })
    store = 'two'; sync.refresh(); await flush(); expect(snapshot).toMatchObject({ storeId: 'two', watermark: 0, status: 'open' })
    sync.stop()
  })
  it('keeps the prefix when reselecting and clears task errors on switching', async () => {
    let fail = false
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: 'one' }),
      sessions: async () => ({ store_id: 'one', sessions: [] }),
      events: async (session, since) => {
        if (fail) throw new Error('old task failed')
        return { store_id: 'one', page: { events: session === 's' && !since ? [event(1)] : [], last_seq: session === 's' ? 1 : 0, status: 'open' } }
      },
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); sync.select('s'); await flush()
    fail = true; sync.refresh(); await flush(); expect(snapshot.error).toContain('old task failed')
    sync.select('s'); expect(snapshot.events).toHaveLength(1)
    sync.select('other'); expect(snapshot.error).toBeNull(); expect(snapshot.events).toHaveLength(0)
    sync.stop()
  })
  it('ignores a list response from the previous store', async () => {
    let store = 'one'
    let release!: (value: Awaited<ReturnType<GatewayClient['sessions']>>) => void
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: store }),
      sessions: () => store === 'one' ? new Promise(resolve => { release = resolve }) : Promise.resolve({ store_id: 'two', sessions: [] }),
      events: async () => ({ store_id: store, page: { events: [], last_seq: 0, status: 'open' } }),
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); store = 'two'; sync.refresh(); await flush()
    release({ store_id: 'one', sessions: [{ session: 'old', title: 'old', project: '', status: 'open', last_seq: 1 }] }); await flush()
    expect(snapshot.storeId).toBe('two'); expect(snapshot.sessions).toEqual([])
    sync.stop()
  })
  it('loads events while a list request is blocked and coalesces repeated triggers', async () => {
    let release!: (value: Awaited<ReturnType<GatewayClient['sessions']>>) => void
    let calls = 0
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: 'one' }),
      sessions: () => { calls++; return calls === 1 ? new Promise(resolve => { release = resolve }) : Promise.resolve({ store_id: 'one', sessions: [] }) },
      events: async () => ({ store_id: 'one', page: { events: [event(1)], last_seq: 1, status: 'open' } }),
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); sync.select('s'); sync.refresh(); sync.refresh(); await flush()
    expect(snapshot.events).toHaveLength(1); expect(calls).toBe(1)
    release({ store_id: 'one', sessions: [] }); await flush(); expect(calls).toBe(2)
    sync.stop()
  })
  it('replays the same graphs and evidence after restarting the synchronizer', async () => {
    const events = scenarios[0].events
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: 'one' }),
      sessions: async () => ({ store_id: 'one', sessions: [] }),
      events: async (_session, since) => ({ store_id: 'one', page: { events: events.slice(since), last_seq: events.length, status: 'done' } }),
      subscribe: async () => () => {},
    }
    const replay = async () => {
      let snapshot!: Snapshot
      const sync = new GatewaySync(client, value => { snapshot = value })
      await sync.start(); await flush(); sync.select(events[0].session); await flush(); sync.stop()
      const projection = projectSession(snapshot.events, snapshot.events.length)
      return { hierarchy: projectGraph(projection, 'hierarchy'), calls: projectGraph(projection, 'calls'), evidence: getNodeEvidence(projection, 'login-api', 'real') }
    }
    const before = await replay(), after = await replay()
    expect(before.hierarchy.nodes.length).toBeGreaterThan(0); expect(before.calls.edges.length).toBeGreaterThan(0); expect(before.evidence.length).toBeGreaterThan(0)
    expect(after).toEqual(before)
  })
  it('polls after a lost notification and clears old events when the store changes', async () => {
    vi.useFakeTimers()
    let store = 'one', count = 1
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: store }),
      sessions: async () => ({ store_id: store, sessions: [] }),
      events: async (_session, since) => ({ store_id: store, page: { events: Array.from({ length: count - since }, (_, i) => event(since + i + 1)), last_seq: count, status: 'open' } }),
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); sync.select('s'); await flush()
    expect(snapshot.events).toHaveLength(1)
    count = 2; await vi.advanceTimersByTimeAsync(1000); await flush()
    expect(snapshot.events).toHaveLength(2)
    store = 'two'; count = 1; await vi.advanceTimersByTimeAsync(1000); await flush()
    expect(snapshot.storeId).toBe('two'); expect(snapshot.changedStore).toBe(true); expect(snapshot.events).toHaveLength(1)
    sync.stop(); vi.useRealTimers()
  })
  it('discards an old selection response', async () => {
    let release!: (value: Awaited<ReturnType<GatewayClient['events']>>) => void
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: 'one' }),
      sessions: async () => ({ store_id: 'one', sessions: [] }),
      events: (session) => session === 's' ? new Promise(resolve => { release = resolve }) : Promise.resolve({ store_id: 'one', page: { events: [], last_seq: 0, status: 'open' } }),
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); sync.select('s'); await flush(); sync.select('other')
    release({ store_id: 'one', page: { events: [event(1)], last_seq: 1, status: 'open' } }); await flush()
    expect(snapshot.selected).toBe('other'); expect(snapshot.events).toEqual([])
    sync.stop()
  })
  it('uses UTF-8 binary ordering for session pagination', async () => {
    const names = Array.from({ length: 199 }, (_, i) => `a${String(i).padStart(3, '0')}`).concat('\uE000')
    const calls: (string | null)[] = []
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: 'one' }),
      sessions: async after => { calls.push(after); return { store_id: 'one', sessions: (after === null ? names : ['𐀀']).map(session => ({ session, title: session, project: '', status: 'open', last_seq: 1 })) } },
      events: async () => ({ store_id: 'one', page: { events: [], last_seq: 0, status: 'open' } }),
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush()
    expect(calls).toEqual([null, '\uE000']); expect(snapshot.sessions).toHaveLength(201); expect(snapshot.error).toBeNull()
    sync.stop()
  })
  it('reads actual cursors and preserves a verified prefix on a corrupt page, then recovers', async () => {
    let corrupt = true
    const client: GatewayClient = {
      status: async () => ({ status: 'ready', store_id: 'one' }),
      sessions: async () => ({ store_id: 'one', sessions: [] }),
      events: vi.fn(async (_session, since) => ({ store_id: 'one', page: since === 0 ? { events: [event(1)], last_seq: 2, status: 'open' } : { events: [event(corrupt ? 3 : 2)], last_seq: 2, status: 'done' } })),
      subscribe: async () => () => {},
    }
    let snapshot!: Snapshot
    const sync = new GatewaySync(client, value => { snapshot = value })
    await sync.start(); await flush(); sync.select('s'); await flush()
    expect(snapshot.events.map(e => e.seq)).toEqual([1]); expect(snapshot.error).toBeTruthy()
    corrupt = false; sync.refresh(); await flush()
    expect(snapshot.events.map(e => e.seq)).toEqual([1, 2]); expect(snapshot.error).toBeNull()
    expect(client.events).toHaveBeenLastCalledWith('s', 1, 200)
    sync.stop()
  })
})

