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

it('keeps Conductor optional and refuses unsafe or incomplete origins', () => {
  expect(nativeConfig(valid).conductorOrigin).toBeUndefined();
  expect(nativeConfig({...valid,VISOR_MOBILE_CONDUCTOR_ORIGIN:'https://conductor.example'}).conductorOrigin).toBe('https://conductor.example');
  expect(() => nativeConfig({VISOR_MOBILE_CONDUCTOR_ORIGIN:'https://conductor.example'})).toThrow();
  for (const origin of ['http://conductor.example','https://user:secret@conductor.example','https://conductor.example/path','https://conductor.example?token=secret']) {
    expect(() => nativeConfig({...valid,VISOR_MOBILE_CONDUCTOR_ORIGIN:origin})).toThrow();
  }
});

it('accepts public Conductor CA configuration and refuses private keys', () => {
  const ca = '-----BEGIN CERTIFICATE-----\nYWJj\n-----END CERTIFICATE-----\n';
  expect(nativeConfig({...valid,VISOR_MOBILE_CONDUCTOR_CA_PEM:ca}).conductorCaPem).toBe(ca);
  expect(() => nativeConfig({VISOR_MOBILE_CONDUCTOR_CA_PEM:ca})).toThrow();
  for (const bad of ['private-key', ca.replaceAll('CERTIFICATE','PRIVATE KEY'), ca+ca]) {
    expect(() => nativeConfig({...valid,VISOR_MOBILE_CONDUCTOR_CA_PEM:bad})).toThrow();
  }
});
