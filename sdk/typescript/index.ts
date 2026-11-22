import { spawn } from 'node:child_process';
import { isAbsolute } from 'node:path';

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export interface EventV1 { v: 1; session: string; seq: number; ts: string; type: string; payload: { [key: string]: Json } }
export interface ProducerError { error: { code: string; message: string; index?: number; last_seq?: number; expected_seq?: number; offending_seq?: number } }
export type SubmitResult = { status: 'queued'; batch_id: string; session: string } | { status: 'committed'; batch_id: string; session: string; receipt: { accepted: number; duplicates: number; last_seq: number } } | ProducerError;
export type FlushResult = { committed: number; pending: number; blocked: number } | ProducerError;

const failed = (): ProducerError => ({ error: { code: 'producer_failed', message: 'The producer could not complete the request.' } });
const invalid = (): ProducerError => ({ error: { code: 'invalid_request', message: 'The events could not be encoded as JSON.' } });
const isObject = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const safeNumber = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
function jsonSafe(value: unknown, seen = new Set<object>()): boolean {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return true;
  if (typeof value === 'number') return Number.isFinite(value) && (!Number.isInteger(value) || Number.isSafeInteger(value));
  if (typeof value !== 'object' || seen.has(value)) return false;
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return false;
  seen.add(value);
  const valid = Array.isArray(value)
    ? Reflect.ownKeys(value).every(key => key === 'length' || (typeof key === 'string' && /^(0|[1-9]\d*)$/.test(key) && Number(key) < value.length)) && Array.from({ length: value.length }, (_, index) => { const descriptor=Object.getOwnPropertyDescriptor(value,index);return descriptor?.enumerable === true && 'value' in descriptor && jsonSafe(descriptor.value,seen); }).every(Boolean)
    : Reflect.ownKeys(value).every(key => typeof key === 'string' && Object.getOwnPropertyDescriptor(value, key)?.enumerable === true && 'value' in Object.getOwnPropertyDescriptor(value, key)! && jsonSafe((value as Record<string, unknown>)[key], seen));
  seen.delete(value); return valid;
}
function uniqueJson(text: string): boolean {
  let i = 0;
  const whitespace = () => { while (i < text.length && /[\x20\t\r\n]/.test(text[i])) i++; };
  const string = (): string => { const start=i++; while(i<text.length) {const c=text[i++];if(c==='\\') {if(i>=text.length) throw Error();i++;}else if(c==='"') return JSON.parse(text.slice(start,i)) as string;}throw Error(); };
  const value = (): void => { whitespace();const c=text[i];if(c==='"'){string();return;}if(c==='{'||c==='['){i++;whitespace();const end=c==='{'?'}':']';if(text[i]===end){i++;return;}const keys=new Set<string>();for(;;){if(c==='{'){whitespace();if(text[i]!=='"')throw Error();const key=string();if(keys.has(key))throw Error();keys.add(key);whitespace();if(text[i++]!==':')throw Error();}value();whitespace();if(text[i]===end){i++;return;}if(text[i++]!==',')throw Error();}}const match=/^(?:true|false|null|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(text.slice(i));if(!match)throw Error();i+=match[0].length;};
  try {value();whitespace();return i===text.length;} catch {return false;}
}
interface ExpectedBatch { session: string; count: number; lastSeq: number }
function expectedBatch(events: EventV1[]): ExpectedBatch | null {
  if (events.length < 1 || events.length > 200) return null;
  let session: string | undefined, lastSeq = 0;
  for (const event of events) {
    if (!isObject(event) || typeof event.session !== 'string' || !event.session.trim() || (session !== undefined && session !== event.session) || !safeNumber(event.seq) || event.seq <= lastSeq) return null;
    session = event.session; lastSeq = event.seq;
  }
  return { session: session!, count: events.length, lastSeq };
}
function validResult(value: unknown, command: 'submit-events' | 'flush', code: number | null, batchId?: string, expected?: ExpectedBatch | null): value is SubmitResult | FlushResult {
  if (!isObject(value)) return false;
  if ('error' in value) {
    if (code !== 1 || Object.keys(value).length !== 1 || !isObject(value.error)) return false;
    const error = value.error;
    return typeof error.code === 'string' && error.code.length > 0 && typeof error.message === 'string' &&
      Object.keys(error).every(key => key === 'code' || key === 'message' || (['index', 'last_seq', 'expected_seq', 'offending_seq'].includes(key) && safeNumber(error[key])));
  }
  if (code !== 0) return false;
  if (command === 'flush') return Object.keys(value).length === 3 && ['committed', 'pending', 'blocked'].every(key => safeNumber(value[key]));
  if (!expected || typeof value.batch_id !== 'string' || value.batch_id !== batchId || value.session !== expected.session) return false;
  if (value.status === 'queued') return Object.keys(value).length === 3;
  if (value.status !== 'committed' || Object.keys(value).length !== 4 || !isObject(value.receipt)) return false;
  const {accepted,duplicates,last_seq}=value.receipt;
  return Object.keys(value.receipt).length === 3 && safeNumber(accepted) && safeNumber(duplicates) && safeNumber(last_seq) && accepted + duplicates === expected.count && last_seq >= expected.lastSeq;
}

export class MindTrailProducer {
  constructor(private readonly executablePath: string, private readonly options: { timeoutMs?: number; maxOutputBytes?: number } = {}) {}
  async submitEvents(queue: string, batchId: string, events: EventV1[]): Promise<SubmitResult> {
    let input: string; try { if (!Array.isArray(events) || !jsonSafe(events)) return invalid(); input = JSON.stringify(events); } catch { return invalid(); }
    return this.run('submit-events', ['--queue', queue, '--batch-id', batchId], input, expectedBatch(JSON.parse(input) as EventV1[])) as Promise<SubmitResult>;
  }
  async flush(queue: string): Promise<FlushResult> { return this.run('flush', ['--queue', queue], '') as Promise<FlushResult>; }
  private run(command: 'submit-events' | 'flush', args: string[], input: string, expected?: ExpectedBatch | null): Promise<SubmitResult | FlushResult> {
    if (!isAbsolute(this.executablePath)) return Promise.resolve(failed());
    const timeout = this.options.timeoutMs ?? 10_000, limit = this.options.maxOutputBytes ?? 65_536;
    if (!Number.isFinite(timeout) || timeout <= 0 || !Number.isSafeInteger(limit) || limit <= 0) return Promise.resolve(failed());
    return new Promise(resolve => {
      let child: ReturnType<typeof spawn>;
      try { child = spawn(this.executablePath, [command, ...args], { shell: false, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] }); }
      catch { resolve(failed()); return; }
      let settled = false, overflow = false, stdoutLength = 0, stderrLength = 0; const stdout: Buffer[] = [];
      const finish = (result: SubmitResult | FlushResult) => { if (!settled) { settled = true; clearTimeout(timer); resolve(result); } };
      const abort = () => { overflow = true; child.kill('SIGKILL'); child.stdin?.destroy(); child.stdout?.destroy(); child.stderr?.destroy(); finish(failed()); };
      const timer = setTimeout(abort, timeout);
      child.on('error', () => finish(failed()));
      child.stdout?.on('data', (chunk: Buffer) => { stdoutLength += chunk.length; if (stdoutLength > limit) abort(); else stdout.push(chunk); });
      child.stderr?.on('data', (chunk: Buffer) => { stderrLength += chunk.length; if (stderrLength > limit) abort(); });
      child.stdin?.on('error', () => {});
      child.on('close', code => {
        if (overflow) { finish(failed()); return; }
        try { const decoded = new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(stdout)); if (!uniqueJson(decoded)) { finish(failed()); return; } const value: unknown = JSON.parse(decoded); finish(validResult(value, command, code, command === 'submit-events' ? args[3] : undefined, expected) ? value : failed()); }
        catch { finish(failed()); }
      });
      child.stdin?.end(input, 'utf8');
    });
  }
}
