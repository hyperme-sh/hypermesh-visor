import React, { useEffect, useRef, useState } from 'react';
import { Conversation, type ConversationView } from './conversation';
import { nativePromptClient } from './native';
export function ConversationPanel({ connected }: { connected: boolean }) {
  const controller = useRef<Conversation | null>(null);
  const [view, setView] = useState<ConversationView>({ sessions: [], selected: '', messages: [], busy: false, error: '' });
  const [text, setText] = useState('');
  const [model, setModel] = useState('');
  const [purpose, setPurpose] = useState('chat');
  useEffect(() => {
    try { controller.current = new Conversation(nativePromptClient(), setView); void controller.current.refresh(); }
    catch { setView(v=>({...v,error:'Open Visor in the mobile app.'})); }
    return () => { controller.current?.dispose(); controller.current=null; };
  }, []);
  useEffect(() => { controller.current?.connection(connected); setText(''); }, [connected]);
  return <section className="card conversation" aria-label="Remote assistant" aria-busy={view.busy}>
    <h2>Talk to your assistant</h2>
    <p>Choose an active assistant you own. Each message uses its current host permissions.</p>
    <label>Assigned assistant<select disabled={view.busy} value={view.selected} onChange={e=>{ controller.current?.select(e.target.value); setText(''); }}>
      <option value="">Choose an assistant</option>
      {view.sessions.map(s=><option key={s.botId} value={s.botId}>Assistant {s.botId.slice(0,8)}</option>)}
    </select></label>
    {!view.busy && !view.sessions.length && <p>No active assigned assistant is available.</p>}
    <button className="secondary" disabled={view.busy} onClick={()=>void controller.current?.refresh()}>Refresh assistants</button>
    <label>Model<input value={model} disabled={view.busy} maxLength={256} onChange={e=>setModel(e.target.value)} placeholder="Model available to this assistant" autoComplete="off"/></label>
    <label>Purpose<select value={purpose} disabled={view.busy} onChange={e=>setPurpose(e.target.value)}><option value="chat">Conversation</option><option value="coding">Coding</option></select></label>
    <div className="transcript" role="log" aria-label="Conversation" aria-live="polite">{view.messages.map((message,i)=><article key={i} className={message.role}><strong>{message.role==='user'?'You':'Assistant'}</strong><p>{message.text}</p></article>)}</div>
    {!connected && <p>Connect this phone to send a message.</p>}
    <form onSubmit={e=>{ e.preventDefault(); const prompt=text; setText(''); void controller.current?.send(prompt,model,purpose); }}>
      <label>Message<textarea value={text} onChange={e=>setText(e.target.value)} disabled={!connected || view.busy} maxLength={65536} rows={4}/></label>
      <button disabled={!connected || view.busy || !view.selected || !text.trim() || !model.trim()}>Send message</button>
    </form>
    {view.busy && <p role="status">Working…</p>}{view.error && <p role="alert" className="message">{view.error}</p>}
  </section>;
}
