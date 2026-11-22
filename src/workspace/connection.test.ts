import { expect, it } from 'vitest'
import { codexMcpConfig } from './connection'

it('quotes Windows and Unicode paths without embedding a token', () => {
  const value=codexMcpConfig('C:\\Program Files\\MindTrail\\producer\\mindtrail-producer.exe','codex')
  expect(value).toContain('command = "C:\\\\Program Files\\\\MindTrail\\\\producer\\\\mindtrail-producer.exe"')
  expect(value).not.toMatch(/token/i)
  expect(codexMcpConfig('C:\\中文目录\\a\'"b.exe','a_1-2')).toContain('中文目录')
})
it('rejects control characters and unsafe Windows queue names', () => {
  for (const queue of ['', 'A', 'con', 'com1', 'lpt9', 'a/b', 'x'.repeat(65)]) expect(() => codexMcpConfig('C:\\ok.exe',queue)).toThrow()
  expect(() => codexMcpConfig('C:\\bad\npath.exe','codex')).toThrow()
})
