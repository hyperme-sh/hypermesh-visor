import { expect, it, vi } from 'vitest';
import { FeedbackClient } from './feedback-client';
import type { VisorFeedbackNative } from './native';
const report = { kind: 'bug' as const, title: 'Example title', description: 'Manually entered description' };
function native(): VisorFeedbackNative {
  return { feedbackStatus: vi.fn(async () => ({ configured: true })), feedbackStart: vi.fn(async () => ({ sessionId: '1' })),
    feedbackCancel: vi.fn(async () => {}), feedbackConsent: vi.fn(async () => ({ enabled: true })), feedbackSetConsent: vi.fn(async () => {}),
    feedbackPrepare: vi.fn(async () => ({ ...report, operationId: 'op' })), feedbackSubmit: vi.fn(async () => ({submitted:true,retryable:false})) };
}
it('never sends during preview and retries with the same native handle', async () => {
  const bridge = native(), client = new FeedbackClient(() => bridge);
  await client.prepare(report); expect(bridge.feedbackSubmit).not.toHaveBeenCalled();
  vi.mocked(bridge.feedbackSubmit).mockRejectedValueOnce(new Error('unknown'));
  await expect(client.submit()).rejects.toThrow(); await client.submit();
  expect(vi.mocked(bridge.feedbackSubmit).mock.calls).toEqual([[{sessionId:'1',operationId:'op'}],[{sessionId:'1',operationId:'op'}]]);
  await expect(client.submit()).rejects.toThrow();
});
it('cancels a late start after withdrawal without preparing text', async () => {
  const bridge = native(); let finish!: (s:{sessionId:string})=>void;
  bridge.feedbackStart = vi.fn(() => new Promise<{sessionId:string}>(resolve => { finish = resolve; }));
  const client = new FeedbackClient(() => bridge), pending = client.prepare(report);
  client.cancel(); finish({sessionId:'old'}); await expect(pending).rejects.toThrow();
  expect(bridge.feedbackCancel).toHaveBeenCalledWith({sessionId:'old'}); expect(bridge.feedbackPrepare).not.toHaveBeenCalled();
});
it('refuses a changed native preview and clears the native handle on edit', async () => {
  const bridge = native(), client = new FeedbackClient(() => bridge);
  vi.mocked(bridge.feedbackPrepare).mockResolvedValueOnce({...report,title:'changed',operationId:'op'});
  await expect(client.prepare(report)).rejects.toThrow(); await expect(client.submit()).rejects.toThrow();
  await client.prepare(report); client.cancel(); await expect(client.submit()).rejects.toThrow();
});
it('withdrawal is explicit and never enables consent automatically', async () => {
  const bridge = native(), client = new FeedbackClient(() => bridge);
  await client.prepare(report); expect(bridge.feedbackSetConsent).not.toHaveBeenCalled();
  await client.consent(false); expect(bridge.feedbackSetConsent).toHaveBeenCalledWith({sessionId:'1',enabled:false});
  await expect(client.submit()).rejects.toThrow();
});

it('requires a new preview after native refusal and retains only retryable operations', async () => {
  const bridge=native(), client=new FeedbackClient(()=>bridge);
  await client.prepare(report);
  vi.mocked(bridge.feedbackSubmit).mockResolvedValueOnce({submitted:false,retryable:true});
  await expect(client.submit()).rejects.toMatchObject({retryable:true});
  vi.mocked(bridge.feedbackSubmit).mockResolvedValueOnce({submitted:false,retryable:false});
  await expect(client.submit()).rejects.toMatchObject({retryable:false});
  await expect(client.submit()).rejects.toThrow('Preview the report again.');
  expect(bridge.feedbackSubmit).toHaveBeenCalledTimes(2);
});

it('late refusal from an old send cannot clear a newly prepared report', async () => {
  const bridge=native(),client=new FeedbackClient(()=>bridge);
  let finish!: (s:{submitted:boolean;retryable:boolean})=>void;
  vi.mocked(bridge.feedbackSubmit).mockImplementationOnce(()=>new Promise(resolve=>{finish=resolve;}));
  await client.prepare(report);
  const old=client.submit();
  client.cancel();
  vi.mocked(bridge.feedbackStart).mockResolvedValueOnce({sessionId:'new'});
  vi.mocked(bridge.feedbackPrepare).mockResolvedValueOnce({...report,operationId:'new-op'});
  await client.prepare(report);
  finish({submitted:false,retryable:false});
  await expect(old).rejects.toThrow('cancelled');
  await client.submit();
  expect(bridge.feedbackSubmit).toHaveBeenLastCalledWith({sessionId:'new',operationId:'new-op'});
});
