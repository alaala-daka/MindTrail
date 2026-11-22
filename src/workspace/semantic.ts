import { invoke } from '@tauri-apps/api/core'

export type SemanticContext = { store_id: string; session_id: string; node_id: string; prefix_seq: number }
export type SemanticEvidence = { id: string; node_id: string; seq: number; text: string }
export type ProviderStatus = { configured: boolean; provider: string | null; model: string | null; endpoint_origin: string | null }
export type SemanticPreview = { ticket_id: string; context: SemanticContext; provider: string; model: string; prompt_version: string; event_count: number; evidence_count: number; byte_count: number; events: unknown[]; evidence: SemanticEvidence[]; expires_at: number }
export type SemanticCitation = { kind: 'quote'; session_id: string; node_id: string; id: string }
export type SemanticResult = { context: SemanticContext; provider: string; model: string; prompt_version: string; sections: { title: string; text: string; citations: SemanticCitation[]; related_node_ids: string[] }[] }

export const semanticStatus = () => invoke<ProviderStatus>('semantic_status')
export const prepareInsight = (context: SemanticContext) => invoke<SemanticPreview>('prepare_insight', { context })
export const generateInsight = (ticketId: string, requestId: string) => invoke<SemanticResult>('generate_insight', { ticketId, requestId })
export const cancelInsight = (requestId: string) => invoke<void>('cancel_insight', { requestId })
export const releaseInsight = (ticketId: string, context: SemanticContext) => invoke<void>('release_insight', { ticketId, context })
export type SemanticClient = { status: typeof semanticStatus; prepare: typeof prepareInsight; generate: typeof generateInsight; cancel: typeof cancelInsight; release: typeof releaseInsight }
export const semanticClient: SemanticClient = { status: semanticStatus, prepare: prepareInsight, generate: generateInsight, cancel: cancelInsight, release: releaseInsight }
