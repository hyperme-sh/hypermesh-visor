import type { CapacitorConfig } from '@capacitor/cli';
const config: CapacitorConfig = {
  appId: 'sh.hyperme.visor.mobile',
  appName: 'Visor',
  webDir: 'dist',
  // Bundle-local UI only: no remote server, cleartext origin or navigation allowlist.
  server: { androidScheme: 'https', hostname: 'localhost' },
};
export default config;
