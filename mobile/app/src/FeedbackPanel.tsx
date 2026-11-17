import React, { useEffect, useReducer } from 'react';
import { emptyDraft, feedbackDraft } from './feedback';

/** Local preparation only; submission awaits the native intake authority contract. */
export function FeedbackPanel() {
  const [draft, dispatch] = useReducer(feedbackDraft, undefined, emptyDraft);
  useEffect(() => {
    const clear = () => dispatch({ type: 'clear' });
    const visibility = () => { if (document.visibilityState !== 'visible') clear(); };
    document.addEventListener('visibilitychange', visibility);
    window.addEventListener('pagehide', clear);
    return () => {
      document.removeEventListener('visibilitychange', visibility);
      window.removeEventListener('pagehide', clear);
    };
  }, []);
  return <section className="card" aria-labelledby="feedback-title">
    <h2 id="feedback-title">Share an idea or report a problem</h2>
    <p>Draft a report and review its text here. Sending is not available yet; nothing leaves this phone.</p>
    <label><input type="checkbox" checked={draft.optedIn}
      onChange={event => dispatch({ type: 'consent', value: event.target.checked })}/>
      I want to draft a report on this phone.</label>
    {draft.optedIn && <>
      <p className="note">Include only what you want to share. Leave out passwords, prompts, file contents and personal information. No screenshots, logs or attachments are collected. Your draft clears when you leave this screen or switch apps.</p>
      <label htmlFor="feedback-kind">Report type</label>
      <select id="feedback-kind" value={draft.kind} onChange={event => dispatch({ type: 'kind', value: event.target.value === 'idea' ? 'idea' : 'bug' })}>
        <option value="bug">Problem</option><option value="idea">Idea</option>
      </select>
      <label htmlFor="feedback-subject">Title</label>
      <input id="feedback-subject" value={draft.title} maxLength={140} autoComplete="off"
        onChange={event => dispatch({ type: 'edit', field: 'title', value: event.target.value })}/>
      <label htmlFor="feedback-description">Description</label>
      <textarea id="feedback-description" value={draft.description} maxLength={2000} autoComplete="off" rows={5}
        onChange={event => dispatch({ type: 'edit', field: 'description', value: event.target.value })}/>
      {draft.error && <p role="alert">{draft.error}</p>}
      <button className="secondary" onClick={() => dispatch({ type: 'preview' })}>Preview report</button>
      {draft.preview && <section aria-label="Report preview">
        <h3>{draft.preview.kind === 'bug' ? 'Problem' : 'Idea'}: {draft.preview.title}</h3>
        <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{draft.preview.description}</p>
        <button disabled aria-describedby="feedback-unavailable">Send report</button>
        <p id="feedback-unavailable">Sending is not available. This preview has not been submitted.</p>
      </section>}
      <button className="secondary" onClick={() => dispatch({ type: 'clear' })}>Discard draft and turn off</button>
    </>}
  </section>;
}
