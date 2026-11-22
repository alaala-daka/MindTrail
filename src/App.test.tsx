import { expect, it, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import App from './App'

vi.mock('./workspace/gateway', () => ({ isTauri: () => true, desktopClient: {}, GatewaySync: class {} }))
it('starts on an honest empty real task page with explicit demo and connection choices', () => {
  const html=renderToStaticMarkup(<App />)
  expect(html).toContain('暂无真实任务'); expect(html).toContain('连接帮助'); expect(html).toContain('演示')
  expect(html).not.toContain('样例解读')
})
