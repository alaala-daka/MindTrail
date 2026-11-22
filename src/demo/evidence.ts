export type DemoEvidence = { sessionId: string; nodeId: string; kind: 'diff' | 'src'; id: string; releasedAtSeq: number; path: string; text: string; firstLine?: number }
export const demoEvidence: readonly DemoEvidence[] = [
  { sessionId: 'auth', nodeId: 'login-api', kind: 'diff', id: 'login-diff', releasedAtSeq: 4, path: 'src/auth/login.ts', text: '- const user = users.find(u => u.email === email)\n+ const user = await findUserByEmail(email)' },
  { sessionId: 'auth', nodeId: 'login-api', kind: 'src', id: 'login-source', releasedAtSeq: 4, path: 'src/auth/login.ts', firstLine: 42, text: 'const user = await findUserByEmail(email)\nconst ok = await verifyPassword(password, user.hash)' },
]
