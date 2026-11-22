import { expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { scenarios } from '../demo/scenarios'
import { Workbench } from './Workbench'
import { appReducer } from './appState'
import { createWorkspace } from './state'
import type { Snapshot } from './gateway'

vi.mock('../graph/GraphCanvas', () => ({ GraphCanvas: () => null }))
vi.mock('./FactDrawer', () => ({ FactDrawer: () => null }))
vi.mock('./InsightDrawer', () => ({ InsightDrawer: () => null }))

it('renders selected status and watermark from the event snapshot rather than stale list data', () => {
  const events = scenarios[0].events
  const snapshot: Snapshot = { storeId: 'one', selected: 'auth', sessions: [{ session: 'auth', title: '真实任务', project: '', status: 'open', last_seq: 1 }], events: [...events], watermark: events.length, status: 'done', error: null, changedStore: false }
  const app = appReducer({ workspace: createWorkspace(scenarios[0]), snapshot: null }, { type: 'snapshot', snapshot })
  const html = renderToStaticMarkup(<Workbench state={app.workspace} snapshot={snapshot} dispatch={() => {}} selectReal={() => {}} />)
  expect(html).toContain(`已完成 · ${events.length}`)
  expect(html).not.toContain('运行中 · 1')
})

it('keeps the known watermark while awaiting the first page and completes only after replay', () => {
  const events = scenarios[0].events
  const snapshot: Snapshot = { storeId: 'one', selected: 'auth', sessions: [{ session: 'auth', title: '真实任务', project: '', status: 'done', last_seq: events.length }], events: [], watermark: 0, status: null, error: null, changedStore: false }
  const render = (next: Snapshot) => {
    const app = appReducer({ workspace: createWorkspace(scenarios[0]), snapshot: null }, { type: 'snapshot', snapshot: next })
    return renderToStaticMarkup(<Workbench state={app.workspace} snapshot={next} dispatch={() => {}} selectReal={() => {}} />)
  }
  const loading = render(snapshot)
  expect(loading).toContain(`补读中 · ${events.length}`)
  expect(loading).not.toContain('运行中 · 0')
  expect(loading).not.toContain('已读取 0')
  const partial = render({ ...snapshot, events: [...events.slice(0, 2)], watermark: events.length, status: 'done' })
  expect(partial).toContain(`补读中 · ${events.length}`)
  const complete = render({ ...snapshot, events: [...events], watermark: events.length, status: 'done' })
  expect(complete).toContain(`已完成 · ${events.length}`)
  expect(complete).not.toContain('补读中')
})

it('shows catch-up when the list advances ahead of an earlier event page, including on error', () => {
  const events = scenarios[0].events
  const snapshot: Snapshot = { storeId: 'one', selected: 'auth', sessions: [{ session: 'auth', title: '真实任务', project: '', status: 'done', last_seq: events.length }], events: [...events.slice(0, 2)], watermark: 2, status: 'open', error: '暂时无法读取', changedStore: false }
  const app = appReducer({ workspace: createWorkspace(scenarios[0]), snapshot: null }, { type: 'snapshot', snapshot })
  const html = renderToStaticMarkup(<Workbench state={app.workspace} snapshot={snapshot} dispatch={() => {}} selectReal={() => {}} />)
  expect(html).toContain(`补读中 · ${events.length}`)
  expect(html).toContain('暂时无法读取')
  expect(html).not.toContain('运行中 · 2')
})
