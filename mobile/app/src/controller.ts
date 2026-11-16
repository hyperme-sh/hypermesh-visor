import { checkedState, type State, type VisorNative, type Workspace } from './native';
export const disconnected: State = { enrolled: false, revoked: false, connection: 'disconnected', canConnect: false };
export interface View { state: State; busy: boolean; userCode: string; workspaces: Workspace[]; message: string }
/** Discard callbacks from cancelled/replaced operations. Native code independently enforces authority. */
export class Controller {
  private generation = 0;
  private view: View = { state: disconnected, busy: false, userCode: '', workspaces: [], message: '' };
  constructor(private native: VisorNative, private changed: (view: View) => void) {}
  private publish(update: Partial<View>) { this.view = { ...this.view, ...update }; this.changed(this.view); }
  private async operation(action: () => Promise<Partial<View>>) {
    const stamp = ++this.generation;
    this.publish({ busy: true, message: '' });
    try { const result = await action(); if (stamp === this.generation) this.publish(result); }
    catch {
      if (stamp === this.generation) {
        // Mutations can persist a safe local refusal before a remote failure.
        // Reconcile native state so reauthentication is available immediately.
        let state: State = { ...this.view.state, connection: 'unreachable', canConnect: false };
        try { state = checkedState(await this.native.status()); } catch { /* Fixed public error below. */ }
        if (stamp === this.generation) this.publish({ state, message: 'That action could not be completed. Please try again.' });
      }
    }
    finally { if (stamp === this.generation) this.publish({ busy: false }); }
  }
  refresh() { return this.operation(async () => ({ state: checkedState(await this.native.status()) })); }
  signIn() { return this.operation(async () => {
    const result = await this.native.beginSignIn();
    if (typeof result.userCode !== 'string' || !/^[A-Za-z0-9 -]{1,128}$/.test(result.userCode)) throw new Error();
    return { userCode: result.userCode, workspaces: [] };
  }); }
  finishSignIn() { return this.operation(async () => {
    const result = await this.native.finishSignIn();
    if (!Array.isArray(result.workspaces) || result.workspaces.length > 100 || result.workspaces.some(w =>
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(w.id) || typeof w.name !== 'string' || !w.name.trim() || w.name.length > 128)) throw new Error();
    if (this.view.state.enrolled && !this.view.state.revoked) return { userCode: '', workspaces: [], state: checkedState(await this.native.status()) };
    return { userCode: '', workspaces: result.workspaces.map(({id,name})=>({id,name})) };
  }); }
  enroll(workspaceId: string) { return this.operation(async () => {
    if (!this.view.workspaces.some(w=>w.id===workspaceId)) throw new Error();
    return { state: checkedState(await this.native.enroll({ workspaceId })), workspaces: [] };
  }); }
  connect() { return this.operation(async () => {
    if (!this.view.state.enrolled || this.view.state.revoked || !this.view.state.canConnect) throw new Error();
    return { state: checkedState(await this.native.connect()) };
  }); }
  disconnect() { return this.operation(async () => ({ state: checkedState(await this.native.disconnect()) })); }
  revoke() { return this.operation(async () => ({ state: checkedState(await this.native.revoke()) })); }
  async cancel() {
    ++this.generation;
    this.publish({ busy: false, userCode: '', workspaces: [] });
    try { await this.native.cancelSignIn(); } catch { this.publish({ message: 'Sign-in could not be cancelled. Close the browser and try again.' }); }
  }
  dispose() { ++this.generation; void this.native.cancelSignIn().catch(() => {}); }
}
