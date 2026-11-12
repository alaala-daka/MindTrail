export type CitationRef = { kind: 'quote' | 'diff' | 'src'; nodeId: string; id: string }
export type SampleInsight = { sessionId: string; nodeId: string; sections: readonly { title: string; text: string; citations: readonly CitationRef[]; relatedNodeIds: readonly string[] }[] }
export const sampleInsights: readonly SampleInsight[] = [{ sessionId: 'auth', nodeId: 'login-api', sections: [
  { title: '它做什么', text: '校验凭据并签发令牌。', citations: [{ kind: 'quote', nodeId: 'login-api', id: 'auth:3' }], relatedNodeIds: [] },
  { title: '它如何实现', text: '查询用户后校验密码。', citations: [{ kind: 'diff', nodeId: 'login-api', id: 'login-diff' }, { kind: 'src', nodeId: 'login-api', id: 'login-source' }], relatedNodeIds: [] },
  { title: '它与谁交互', text: '登录接口调用处理程序，处理程序引入密码校验。', citations: [], relatedNodeIds: ['handler', 'password'] },
] }]
