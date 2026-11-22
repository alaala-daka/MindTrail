export type DataSource = 'real' | 'demo'
export type CitationRef = { kind: 'quote' | 'diff' | 'src'; nodeId: string; id: string }
export type InsightSection = { title: string; text: string; citations: readonly CitationRef[]; relatedNodeIds: readonly string[] }
export type Insight = { sessionId: string; nodeId: string; sections: readonly InsightSection[] }
