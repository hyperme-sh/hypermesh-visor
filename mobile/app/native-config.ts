export function nativeConfig(env: Record<string, string | undefined>): Partial<Record<'issuer' | 'authOrigin' | 'planeOrigin' | 'workspaceId' | 'workspaceName' | 'conductorOrigin', string>> {
  const names = ['issuer', 'authOrigin', 'planeOrigin', 'workspaceId', 'workspaceName'] as const;
  const keys = ['VISOR_MOBILE_ISSUER', 'VISOR_MOBILE_AUTH_ORIGIN', 'VISOR_MOBILE_PLANE_ORIGIN', 'VISOR_MOBILE_WORKSPACE_ID', 'VISOR_MOBILE_WORKSPACE_NAME'];
  const values = keys.map(key => env[key]);
  if (values.every(value => !value)) {
    if (env.VISOR_MOBILE_CONDUCTOR_ORIGIN) throw new Error('Complete native phone configuration is required.');
    return {};
  }
  if (values.some(value => !value || /[\x00-\x1f\x7f]/.test(value))) throw new Error('Complete native phone configuration is required.');
  const config = Object.fromEntries(names.map((name, index) => [name, values[index]!])) as Record<typeof names[number], string>;
  for (const key of ['issuer', 'authOrigin', 'planeOrigin'] as const) {
    const url = new URL(config[key]);
    if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash || (key !== 'issuer' && url.pathname !== '/')) {
      throw new Error('Native service configuration must use HTTPS without credentials or redirects.');
    }
  }
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(config.workspaceId) || !config.workspaceName.trim() || config.workspaceName.length > 128) {
    throw new Error('Invalid workspace configuration.');
  }
  const conductor = env.VISOR_MOBILE_CONDUCTOR_ORIGIN;
  if (conductor) {
    const url = new URL(conductor);
    if (/[\x00-\x1f\x7f]/.test(conductor) || url.protocol !== 'https:' || url.username || url.password || url.search || url.hash || url.pathname !== '/') {
      throw new Error('Conductor configuration must be an HTTPS origin.');
    }
  }
  return { ...config, ...(conductor ? { conductorOrigin: conductor } : {}) };
}
