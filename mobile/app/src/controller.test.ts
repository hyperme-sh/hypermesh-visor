import { describe, expect, it, vi } from 'vitest';
import { Controller, disconnected, type View } from './controller';
import type { VisorNative } from './native';
function setup() {
  const native: VisorNative = {
    status: vi.fn(async()=>disconnected), beginSignIn: vi.fn(async()=>({userCode:'ABCD-1234'})),
    finishSignIn: vi.fn(async()=>({workspaces:[]})), cancelSignIn:vi.fn(async()=>{}),
    enroll:vi.fn(async()=>disconnected),renew:vi.fn(async()=>disconnected),revoke:vi.fn(async()=>disconnected),
    connect:vi.fn(async()=>disconnected),disconnect:vi.fn(async()=>disconnected),
  };
  let view: View | undefined;
  const controller = new Controller(native, v=>{view=v;});
  return {native,controller,view:()=>view!};
}
describe('native UI contract',()=>{
  it('refuses connection and arbitrary workspace selection without authority',async()=>{
    const s=setup();await s.controller.refresh();await s.controller.connect();await s.controller.enroll('unknown');
    expect(s.native.connect).not.toHaveBeenCalled();expect(s.native.enroll).not.toHaveBeenCalled();
  });
  it('discards a completed sign-in callback after cancellation',async()=>{
    const s=setup();let release!:(v:{workspaces:[]})=>void;
    s.native.finishSignIn=vi.fn(()=>new Promise<{workspaces:[]}>(resolve=>{release=resolve;}));
    await s.controller.signIn();const pending=s.controller.finishSignIn();await s.controller.cancel();
    release({workspaces:[]});await pending;expect(s.view().userCode).toBe('');expect(s.view().busy).toBe(false);
  });
  it('drops unexpected native fields and never shows raw failure text',async()=>{
    const s=setup();s.native.status=vi.fn(async()=>({...disconnected,privateKey:'secret-canary'}));
    await s.controller.refresh();expect(JSON.stringify(s.view())).not.toContain('secret-canary');
    s.native.beginSignIn=vi.fn(async()=>{throw new Error('credential-canary');});
    await s.controller.signIn();expect(JSON.stringify(s.view())).not.toContain('credential-canary');
  });
});
