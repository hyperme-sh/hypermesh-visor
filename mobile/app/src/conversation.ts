import type { PhoneSession, VisorPromptNative } from './native';
export interface Message { role: 'user' | 'assistant'; text: string }
export interface ConversationView { sessions: PhoneSession[]; selected: string; messages: Message[]; busy: boolean; error: string }
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export class Conversation {
  private generation = 0;
  private connected = false;
  private view: ConversationView = { sessions: [], selected: '', messages: [], busy: false, error: '' };
  constructor(private native: VisorPromptNative, private changed: (v: ConversationView) => void) {}
  private publish(update: Partial<ConversationView>) { this.view = { ...this.view, ...update }; this.changed(this.view); }
  connection(connected: boolean) {
    if (this.connected === connected) return;
    this.connected = connected;
    if (!connected) {
      ++this.generation; this.publish({ messages: [], busy: false, error: '' });
      void this.native.cancelPhone().catch(() => {});
    }
  }
  async refresh() {
    if (this.view.busy) return;
    const stamp = ++this.generation; this.publish({ busy: true, error: '' });
    try {
      const { sessions } = await this.native.ownedSessions();
      if (!Array.isArray(sessions) || sessions.length > 100 || sessions.some(s =>
        ![s.botId,s.assignmentId,s.sessionId].every(id => uuid.test(id) && id !== '00000000-0000-0000-0000-000000000000') ||
        !Number.isSafeInteger(s.expiresAt) || s.expiresAt <= Date.now()/1000)) throw new Error();
      if (stamp === this.generation) this.publish({ sessions, selected: '', messages: [] });
    } catch { if (stamp === this.generation) this.publish({ sessions: [], selected: '', messages: [], error: 'Assigned assistants are unavailable. Sign in again if your session has expired.' }); }
    finally { if (stamp === this.generation) this.publish({ busy: false }); }
  }
  select(id: string) {
    if (this.view.busy || !this.view.sessions.some(s => s.botId === id)) return;
    ++this.generation; this.publish({ selected: id, messages: [], error: '' });
  }
  async send(text: string, destination: string, taskClass: string) {
    const selected = this.view.sessions.find(s => s.botId === this.view.selected);
    if (!this.connected || this.view.busy || !selected || selected.expiresAt <= Date.now()/1000 ||
      !text.trim() || new TextEncoder().encode(text).length > 65536 || !destination.trim() || destination.length > 256 ||
      !['chat','coding'].includes(taskClass)) return;
    const stamp = ++this.generation;
    this.publish({ busy: true, error: '', messages: [...this.view.messages.slice(-48), { role: 'user', text }] });
    try {
      const result = await this.native.sendPrompt({ ...selected, taskClass, destination, prompt: text, maxOutputTokens: 2048 });
      if (typeof result.text !== 'string' || new TextEncoder().encode(result.text).length > 262144) throw new Error();
      if (stamp === this.generation) this.publish({ messages: [...this.view.messages, { role: 'assistant', text: result.text }] });
    } catch { if (stamp === this.generation) this.publish({ error: 'No reply was confirmed. The request may have run; it has not been sent again.' }); }
    finally { if (stamp === this.generation) this.publish({ busy: false }); }
  }
  dispose() { ++this.generation; this.connected = false; void this.native.cancelPhone().catch(() => {}); }
}
