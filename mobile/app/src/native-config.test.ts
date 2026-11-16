import { describe, expect, it } from 'vitest';
import { nativeConfig } from '../native-config';
const valid = {
  VISOR_MOBILE_ISSUER: 'https://identity.example/realms/public',
  VISOR_MOBILE_AUTH_ORIGIN: 'https://auth.example',
  VISOR_MOBILE_PLANE_ORIGIN: 'https://plane.example',
  VISOR_MOBILE_WORKSPACE_ID: 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
  VISOR_MOBILE_WORKSPACE_NAME: 'Workspace',
};
describe('native-only service configuration', () => {
  it('allows an unconfigured build without inventing live origins', () => expect(nativeConfig({})).toEqual({}));
  it('accepts only complete HTTPS configuration with a canonical target', () => {
    expect(nativeConfig(valid).workspaceId).toBe(valid.VISOR_MOBILE_WORKSPACE_ID);
    expect(() => nativeConfig({VISOR_MOBILE_ISSUER: valid.VISOR_MOBILE_ISSUER})).toThrow();
    for (const url of ['http://plane.example', 'https://user:secret@plane.example', 'https://plane.example/path', 'https://plane.example?token=secret']) {
      expect(() => nativeConfig({...valid, VISOR_MOBILE_PLANE_ORIGIN:url})).toThrow();
    }
  });
});
