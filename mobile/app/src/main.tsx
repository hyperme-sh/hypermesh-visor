import React, { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Controller, disconnected, type View } from './controller';
import { nativeClient } from './native';
import './style.css';
import { ConversationPanel } from './ConversationPanel';
function App() {
  const [view, setView] = useState<View>({ state: disconnected, busy: true, userCode: '', workspaces: [], message: '' });
  const controller = useRef<Controller | null>(null);
  useEffect(() => {
    try { controller.current = new Controller(nativeClient(), setView); void controller.current.refresh(); }
    catch { setView(v=>({...v,busy:false,message:'Open Visor in the mobile app.'})); }
    const timer = window.setInterval(() => void controller.current?.poll(), 2000);
    return () => { window.clearInterval(timer); controller.current?.dispose(); controller.current=null; };
  }, []);
  const state=view.state;
  const active = ['connected', 'connecting', 'reconnecting'].includes(state.connection);
  return <main>
    <header><span className="brand">VISOR</span><span className={`status ${state.connection}`}><i/>{state.connection}</span></header>
    <section className="intro"><p className="eyebrow">YOUR NETWORK, WITH YOU</p><h1>A place for<br/>your assistants.</h1><p>Connect this phone to your Hypermesh workspace.</p></section>
    <section className="card" aria-busy={view.busy}>
      <h2>{state.revoked?'This phone is disconnected':state.enrolled?'This phone is enrolled':'Make yourself at home'}</h2>
      {view.userCode ? <><p>Complete sign-in in your browser using this code.</p><output className="code">{view.userCode}</output><button disabled={view.busy} onClick={()=>void controller.current?.finishSignIn()}>I’ve signed in</button><button className="secondary" onClick={()=>void controller.current?.cancel()}>Cancel</button></>
      : view.workspaces.length ? <><p>Choose your workspace.</p>{view.workspaces.map(w=><button className="workspace" key={w.id} disabled={view.busy} onClick={()=>void controller.current?.enroll(w.id)}>{w.name}<span aria-hidden>→</span></button>)}<button className="secondary" disabled={view.busy} onClick={()=>void controller.current?.cancel()}>Back to sign in</button></>
      : state.enrolled ? <><p>{state.connection==='connected'?'Connected to your network.':'Your device identity is stored securely on this phone.'}</p>
        <button disabled={view.busy || (!state.canConnect && !active)} onClick={()=>void (active?controller.current?.disconnect():controller.current?.connect())}>{active?'Disconnect':'Connect'}</button>
        {!state.canConnect && !active && <p className="note">Connection is currently unavailable.</p>}
        <button className="secondary" disabled={view.busy} onClick={()=>void controller.current?.signIn()}>Sign in again</button>
        <button className="secondary" disabled={view.busy} onClick={()=>{ if(window.confirm('Revoke this phone’s network access?')) void controller.current?.revoke(); }}>Remove this phone</button></>
      : <><p>Sign in with your account, then choose a workspace. Your private keys stay on this device.</p><button disabled={view.busy || !controller.current} onClick={()=>void controller.current?.signIn()}>Sign in</button></>}
      {view.busy && <p role="status">Working…</p>}
      {view.message && <p className="message" role="alert">{view.message}</p>}
    </section>{state.enrolled && !state.revoked && !view.busy && !view.userCode && !view.workspaces.length && <ConversationPanel connected={state.connection==='connected'}/> }<footer>Private by default. Connected on your terms.</footer>
  </main>;
}
createRoot(document.getElementById('root')!).render(<App/>);
