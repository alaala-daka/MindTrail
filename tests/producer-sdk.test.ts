import { beforeAll, describe, expect, it } from 'vitest';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { MindTrailProducer, type EventV1 } from '../sdk/typescript/index';

let executable: string;
const event: EventV1 = { v: 1, session: 's', seq: 1, ts: '2024-01-01T00:00:00Z', type: 'task.start', payload: { title: '标题', goal: 'G', project: '../path' } };
beforeAll(() => {
  const directory = resolve('.tools', `sdk fixture ${randomUUID()}`); mkdirSync(directory, { recursive: true });
  executable = resolve(directory, 'fake producer.exe');
  execFileSync(process.env.MINDTRAIL_RUSTC ?? 'rustc', [resolve('tests/fixtures/producer_fixture.rs'), '-o', executable], { timeout: 60_000 });
});
describe('TypeScript producer bridge', () => {
  it('passes a shell metacharacter batch ID and exact UTF-8 input', async () => {
    const id = 'id & echo DO_NOT_RUN | x'; process.env.MINDTRAIL_EXPECT_STDIN = JSON.stringify([event]);
    try { expect(await new MindTrailProducer(executable).submitEvents('q', id, [event])).toEqual({ status: 'queued', batch_id: id, session: 's' }); }
    finally { delete process.env.MINDTRAIL_EXPECT_STDIN; }
  });
  it('preserves CLI errors and flush counts', async () => {
    const sdk = new MindTrailProducer(executable);
    expect(await sdk.submitEvents('q', 'test-error', [event])).toEqual({ error: { code: 'seq_gap', message: 'missing', expected_seq: 2, offending_seq: 3, last_seq: 1, index: 0 } });
    expect(await sdk.flush('q')).toEqual({ committed: 1, pending: 2, blocked: 3 });
    expect(await sdk.submitEvents('q', 'valid-committed', [event])).toEqual({ status: 'committed', batch_id: 'valid-committed', session: 's', receipt: { accepted: 1, duplicates: 0, last_seq: 1 } });
  });
  it('reports malformed, multiple, duplicate, oversized and abnormal outputs', async () => {
    const sdk = new MindTrailProducer(executable, { maxOutputBytes: 2048 });
    for (const id of ['bad-json', 'duplicate', 'bad-shape', 'wrong-id', 'wrong-session', 'wrong-count', 'low-watermark', 'error-zero', 'error-two', 'invalid-utf8', 'exit', 'flood', 'stderr-flood']) expect(await sdk.submitEvents('q', id, [event])).toMatchObject({ error: { code: 'producer_failed' } });
    expect(await new MindTrailProducer(resolve('missing-producer.exe')).flush('q')).toMatchObject({ error: { code: 'producer_failed' } });
    expect(await new MindTrailProducer('relative.exe').flush('q')).toMatchObject({ error: { code: 'producer_failed' } });
  });
  it('kills timed-out children and rejects non-finite or altered JSON', async () => {
    const sdk = new MindTrailProducer(executable, { timeoutMs: 150 });
    const marker = resolve(executable + '.survived'); process.env.MINDTRAIL_FIXTURE_SURVIVAL_MARKER = marker;
    try { expect(await sdk.submitEvents('q', 'hang', [event])).toMatchObject({ error: { code: 'producer_failed' } }); await new Promise(resolve => setTimeout(resolve, 1000)); expect(existsSync(marker)).toBe(false); }
    finally { delete process.env.MINDTRAIL_FIXTURE_SURVIVAL_MARKER; }
    for (const value of [NaN, Infinity, -Infinity, undefined]) expect(await sdk.submitEvents('q', 'x', [{ ...event, payload: { value } } as EventV1])).toMatchObject({ error: { code: 'invalid_request' } });
    const sparse = new Array(1); expect(await sdk.submitEvents('q', 'x', sparse)).toMatchObject({ error: { code: 'invalid_request' } });
  });
});
