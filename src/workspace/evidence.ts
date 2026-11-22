import { demoEvidence } from '../demo/evidence'
import type { CitationRef, DataSource, Insight } from '../domain/insights'
import type { SessionProjection } from '../domain/project'
export type EvidenceItem = { kind: 'quote' | 'diff' | 'src'; id: string; nodeId: string; text: string; ts?: string; context?: string; path?: string; firstLine?: number }
export function getNodeEvidence(session: SessionProjection, nodeId: string, source: DataSource): readonly EvidenceItem[] {
  const node = session.nodes.get(nodeId)
  if (!node) return []
  const quotes: EvidenceItem[] = node.events.filter(event => event.payload.quote).map(event => ({ kind: 'quote', id: `${event.session}:${event.seq}`, nodeId, text: event.payload.quote!, ts: event.ts, context: event.payload.context }))
  const materials: EvidenceItem[] = source === 'demo' ? demoEvidence.filter(item => item.sessionId === session.id && item.nodeId === nodeId && item.releasedAtSeq <= session.lastSeq).map(item => ({ kind: item.kind, id: item.id, nodeId, text: item.text, path: item.path, firstLine: item.firstLine })) : []
  return [...quotes, ...materials]
}
export function resolveCitation(session: SessionProjection, insight: Insight, ref: CitationRef, source: DataSource): EvidenceItem | null {
  if (session.id !== insight.sessionId || !session.nodes.has(insight.nodeId) || !insight.sections.some(section => section.citations.some(item => item.kind === ref.kind && item.nodeId === ref.nodeId && item.id === ref.id))) return null
  return getNodeEvidence(session, ref.nodeId, source).find(item => item.kind === ref.kind && item.id === ref.id) ?? null
}
