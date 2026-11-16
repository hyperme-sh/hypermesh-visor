import { describe, expect, it, vi } from 'vitest';
import { Conversation, type ConversationView } from './conversation';
import type { VisorPromptNative } from './native';
const id = '11111111-1111-4111-8111-111111111111';
const session = { botId:id, assignmentId:id, sessionId:id, expiresAt:Math.floor(Date.now()/1000)+300 };
function fixture(send = vi.fn().mockResolvedValue({text:'Reply'})) {
  let view: ConversationView;
  const native = { ownedSessions:vi.fn().mockResolvedValue({sessions:[session]}), sendPrompt:send, cancelPhone:vi.fn().mockResolvedValue(undefined) };
  const controller = new Conversation(native as unknown as VisorPromptNative, v=>{view=v;});
  return { controller,native,view:()=>view! };
}
describe('phone conversation',()=>{
  it('requires a current selected assignment and connection',async()=>{
    const f=fixture(); await f.controller.refresh(); f.controller.select(id);
    await f.controller.send('Hello','model','chat'); expect(f.native.sendPrompt).not.toHaveBeenCalled();
    f.controller.connection(true); await f.controller.send('Hello','model','chat');
    expect(f.native.sendPrompt).toHaveBeenCalledWith({...session,prompt:'Hello',destination:'model',taskClass:'chat',maxOutputTokens:2048});
    expect(f.view().messages.map(m=>m.text)).toEqual(['Hello','Reply']);
  });
  it('does not retry an uncertain send',async()=>{
    const f=fixture(vi.fn().mockRejectedValue(new Error('private diagnostic')));
    await f.controller.refresh(); f.controller.select(id); f.controller.connection(true);
    await f.controller.send('Hello','model','chat');
    expect(f.native.sendPrompt).toHaveBeenCalledTimes(1); expect(f.view().error).toContain('has not been sent again');
    expect(f.view().error).not.toContain('private');
  });
  it('clears conversation and discards late replies on disconnect',async()=>{
    let resolve!:(v:{text:string})=>void;
    const f=fixture(vi.fn().mockImplementation(()=>new Promise(r=>{resolve=r;})));
    await f.controller.refresh(); f.controller.select(id); f.controller.connection(true);
    const send=f.controller.send('Hello','model','chat'); f.controller.connection(false);
    resolve({text:'stale reply'}); await send;
    expect(f.view().messages).toEqual([]); expect(f.native.cancelPhone).toHaveBeenCalledOnce();
  });
  it('does not discard initial assignment loading when connected',async()=>{
    const f=fixture(); const loading=f.controller.refresh(); f.controller.connection(true); await loading;
    expect(f.view().sessions).toHaveLength(1);
  });
  it('refuses malformed or expired session metadata',async()=>{
    const f=fixture(); f.native.ownedSessions.mockResolvedValue({sessions:[{...session,expiresAt:0}]});
    await f.controller.refresh(); expect(f.view().sessions).toEqual([]); expect(f.view().error).not.toBe('');
  });
  it('keeps drafts when an assignment expires or UTF-8 input is too large',async()=>{
    const f=fixture(); await f.controller.refresh(); f.controller.select(id); f.controller.connection(true);
    const accepted=vi.fn();
    await f.controller.send('😀'.repeat(20000),'model','chat',accepted);
    expect(accepted).not.toHaveBeenCalled(); expect(f.view().error).toContain('64 KB');
    const clock=vi.spyOn(Date,'now').mockReturnValue((session.expiresAt+1)*1000);
    try {
      await f.controller.send('Kept draft','model','chat',accepted);
      expect(accepted).not.toHaveBeenCalled(); expect(f.view().error).toContain('expired');
      expect(f.native.sendPrompt).not.toHaveBeenCalled();
    } finally { clock.mockRestore(); }
  });

  it('accepts named models with digits and refuses control characters',async()=>{
    const f=fixture(); await f.controller.refresh(); f.controller.select(id); f.controller.connection(true);
    for (const model of ['gpt-4o','qwen3','flux']) await f.controller.send('Hello',model,'chat');
    expect(f.native.sendPrompt).toHaveBeenCalledTimes(3);
    await f.controller.send('Hello','model'+String.fromCharCode(10),'chat');
    expect(f.native.sendPrompt).toHaveBeenCalledTimes(3); expect(f.view().error).toContain('valid model');
  });

});
