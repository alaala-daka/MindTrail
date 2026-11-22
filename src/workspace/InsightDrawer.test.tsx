import { expect, it } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import { scenarios } from '../demo/scenarios'
import { projectSession } from '../domain/project'
import { InsightDrawer } from './InsightDrawer'
import { FactDrawer } from './FactDrawer'

it('does not show demo insights or bundled evidence for colliding real identifiers', () => {
  const session = projectSession(scenarios[0].events, 11)
  const base = { session, nodeId: 'login-api', openSeq: 1, onClose() {}, onLocateNode() {}, onRevealEvidence() {} }
  const real = renderToStaticMarkup(<InsightDrawer {...base} source="real" />)
  const demo = renderToStaticMarkup(<InsightDrawer {...base} source="demo" />)
  expect(real).toContain('事实清单')
  expect(real).not.toContain('样例解读')
  expect(demo).toContain('样例解读')
  const facts = renderToStaticMarkup(<FactDrawer session={session} source="real" selectedId="login-api" expanded tab="diff" onToggle={() => {}} onTab={() => {}} onLocate={() => {}} onOpenInsight={() => {}} />)
  expect(facts).not.toContain('随包材料')
})
