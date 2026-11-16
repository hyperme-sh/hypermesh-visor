import { Capacitor, registerPlugin } from '@capacitor/core';

export type Connection = 'disconnected' | 'connecting' | 'connected' | 'reconnecting' | 'unreachable';
export interface State {
  enrolled: boolean;
  revoked: boolean;
  connection: Connection;
  canConnect: boolean;
}
export interface Workspace { id: string; name: string }
export interface VisorNative {
  status(): Promise<State>;
  beginSignIn(): Promise<{ userCode: string }>;
  finishSignIn(): Promise<{ workspaces: Workspace[] }>;
  cancelSignIn(): Promise<void>;
  enroll(options: { workspaceId: string }): Promise<State>;
  renew(): Promise<State>;
  revoke(): Promise<State>;
  connect(): Promise<State>;
  disconnect(): Promise<State>;
}
export interface PhonePromptInput {
  botId: string; assignmentId: string; sessionId: string;
  taskClass: string; destination: string; prompt: string; maxOutputTokens: number;
}
export interface PreparedPhonePrompt { operationId: string; taskId: string; requestSha256: string }
export interface VisorPromptNative extends VisorNative {
  preparePrompt(options: PhonePromptInput): Promise<PreparedPhonePrompt>;
  submitPrompt(options: { operationId: string }): Promise<{ text: string }>;
  discardPrompt(options: { operationId: string }): Promise<void>;
}
const plugin = registerPlugin<VisorPromptNative>('Visor');
export function nativePromptClient(): VisorPromptNative {
  if (!Capacitor.isNativePlatform()) throw new Error('Open Visor in the mobile app.');
  return plugin;
}
// No generic HTTP, filesystem, credential, URL-opening or script-execution bridge.
export function nativeClient(): VisorNative {
  if (!Capacitor.isNativePlatform()) throw new Error('Open Visor in the mobile app.');
  return plugin;
}
export function checkedState(value: State): State {
  if (typeof value?.enrolled !== 'boolean' || typeof value.revoked !== 'boolean' || typeof value.canConnect !== 'boolean'
      || !['disconnected','connecting','connected','reconnecting','unreachable'].includes(value.connection)) {
    throw new Error('Connection status is unavailable.');
  }
  return { enrolled: value.enrolled, revoked: value.revoked, canConnect: value.canConnect, connection: value.connection };
}
