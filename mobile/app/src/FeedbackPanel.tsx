import React, { useEffect, useReducer, useRef, useState } from 'react';
import { emptyDraft, feedbackDraft } from './feedback';
import { FeedbackClient } from './feedback-client';
import { nativeFeedbackClient, type FeedbackPreview } from './native';

/** Drafts stay in memory; native code owns authority and exact submitted bytes. */
export function FeedbackPanel() {
  const [draft, dispatch] = useReducer(feedbackDraft, undefined, emptyDraft);
  const client = useRef(new FeedbackClient(nativeFeedbackClient));
  const generation = useRef(0);
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirmed, setConfirmed] = useState<FeedbackPreview | null>(null);
  const [message, setMessage] = useState('');
  const [uncertain, setUncertain] = useState(false);
  const invalidate = () => { generation.current++; client.current.cancel(); setConfirmed(null); setBusy(false); setUncertain(false); };
  const clear = () => { invalidate(); dispatch({ type: 'clear' }); setMessage(''); };
  useEffect(() => {
    let active = true;
    try { void nativeFeedbackClient().feedbackStatus().then(s => { if (active) setConfigured(s.configured === true); }).catch(() => {}); } catch { /* Unconfigured web preview is local only. */ }

    const visibility = () => { if (document.visibilityState !== 'visible') clear(); };
    document.addEventListener('visibilitychange', visibility);
    window.addEventListener('pagehide', clear);
    return () => {
      document.removeEventListener('visibilitychange', visibility);
      window.removeEventListener('pagehide', clear);
      active = false; generation.current++; client.current.cancel();
    };
  }, []);
  const consent = async (enabled: boolean) => {
    invalidate();
    const op = generation.current;
    if (!enabled) dispatch({ type: 'clear' });
    setBusy(true); setMessage('');
    try { await client.current.consent(enabled); if (op === generation.current) setMessage(enabled ? 'Report submission enabled. Preview before sending.' : 'Report consent withdrawn.'); }
    catch { if (op === generation.current) setMessage(enabled ? 'Could not enable report submission.' : 'Local draft cleared; server withdrawal is not confirmed.'); }
    finally { if (op === generation.current) setBusy(false); }
  };
  const preview = async () => {
    invalidate(); setMessage('');
    const next = feedbackDraft(draft, { type: 'preview' }); dispatch({ type: 'preview' });
    if (!next.preview || !configured) return;
    const op = generation.current; setBusy(true);
    try { const result = await client.current.prepare(next.preview); if (op === generation.current) setConfirmed(result); }
    catch { if (op === generation.current) setMessage('Preview could not be authorized. Enable report submission and try again.'); }
    finally { if (op === generation.current) setBusy(false); }
  };
  const send = async () => {
    if (!confirmed || busy) return;
    const op = generation.current; setBusy(true); setMessage('');
    try { await client.current.submit(); if (op === generation.current) { clear(); setMessage('Report submitted for review.'); } }
    catch { if (op === generation.current) { setUncertain(true); setMessage('Submission is not confirmed. Retry this unchanged report or discard it.'); } }
    finally { if (op === generation.current) setBusy(false); }
  };
  return <section className="card" aria-labelledby="feedback-title">
    <h2 id="feedback-title">Share an idea or report a problem</h2>
    <p>Draft a report and review its text here. No report leaves this phone until you explicitly send it.</p>
    <label><input type="checkbox" checked={draft.optedIn}
      onChange={event => { if (!event.target.checked && configured) void consent(false); else { invalidate(); dispatch({ type: 'consent', value: event.target.checked }); } }}/>
      I opt in to human reports on this phone.</label>
    {message && <p role="status">{message}</p>}
    {draft.optedIn && <>
      {configured && <button disabled={busy} onClick={() => void consent(true)}>Enable report submission</button>}
      <p className="note">Include only what you want to share. Leave out passwords, prompts, file contents and personal information. No screenshots, logs or attachments are collected. Your draft clears when you leave this screen or switch apps.</p>
      <label htmlFor="feedback-kind">Report type</label>
      <select id="feedback-kind" value={draft.kind} onChange={event => { invalidate(); dispatch({ type: 'kind', value: event.target.value === 'idea' ? 'idea' : 'bug' }); }}>
        <option value="bug">Problem</option><option value="idea">Idea</option>
      </select>
      <label htmlFor="feedback-subject">Title</label>
      <input id="feedback-subject" value={draft.title} maxLength={140} autoComplete="off"
        onChange={event => { invalidate(); dispatch({ type: 'edit', field: 'title', value: event.target.value }); }}/>
      <label htmlFor="feedback-description">Description</label>
      <textarea id="feedback-description" value={draft.description} maxLength={2000} autoComplete="off" rows={5}
        onChange={event => { invalidate(); dispatch({ type: 'edit', field: 'description', value: event.target.value }); }}/>
      {draft.error && <p role="alert">{draft.error}</p>}
      <button className="secondary" disabled={busy} onClick={() => void preview()}>Preview report</button>
      {draft.preview && <section aria-label="Report preview">
        <h3>{draft.preview.kind === 'bug' ? 'Problem' : 'Idea'}: {draft.preview.title}</h3>
        <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{draft.preview.description}</p>
        <button disabled={busy || !confirmed} onClick={() => void send()}>{uncertain ? 'Retry unchanged report' : 'Confirm and send report'}</button>
        {!configured && <p>Sending is not configured. This preview has not been submitted.</p>}
      </section>}
      <button className="secondary" onClick={clear}>Discard draft</button>
    </>}
  </section>;
}
