import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
const { MindTrailProducer } = await import(pathToFileURL(resolve(process.argv[2])).href);
const sdk = new MindTrailProducer(resolve(process.argv[3]));
const online = process.argv[4] === 'online';
const event = { v: 1, session: 'sdk-node', seq: 1, ts: '2024-01-01T00:00:00Z', type: 'task.start', payload: { title: 'Node', goal: 'G', project: 'P' } };
const first = await sdk.submitEvents('node', 'node & | batch', [event]);
if (first.status !== (online ? 'committed' : 'queued')) throw Error('unexpected submit result');
const retry = await sdk.submitEvents('node', 'node & | batch', [event]);
if (JSON.stringify(retry) !== JSON.stringify(first)) throw Error('retry changed stored result');
if (online) {
  const conflict = await sdk.submitEvents('node', 'node & | batch', [{ ...event, payload: { ...event.payload, title: 'changed' } }]);
  if (conflict.error?.code !== 'batch_conflict') throw Error('conflict was not returned');
  const counts = await sdk.flush('node'); if (counts.committed !== 1 || counts.pending !== 0 || counts.blocked !== 0) throw Error('bad flush counts');
}
process.stdout.write('PASS node SDK\n');
