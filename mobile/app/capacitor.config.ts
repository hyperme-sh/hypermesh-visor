import { env } from 'node:process';
import { nativeConfig } from './native-config';
import type { CapacitorConfig } from '@capacitor/cli';
const config: CapacitorConfig = {
  appId: 'sh.hyperme.visor.mobile',
  appName: 'Visor',
  webDir: 'dist',
  plugins: { Visor: nativeConfig(env) },
  // Bundle-local UI only: no remote server, cleartext origin or navigation allowlist.
  server: { androidScheme: 'https', hostname: 'localhost' },
};
export default config;
