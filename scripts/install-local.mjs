// `npm run app:install` (apps/desktop): builds and installs Dev All for this user — Start Menu on
// Windows (install-local.ps1), ~/Applications on macOS (install-local.sh). Extra args pass through.
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const run =
  process.platform === 'win32'
    ? spawnSync('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(here, 'install-local.ps1'), ...args], { stdio: 'inherit' })
    : process.platform === 'darwin'
      ? spawnSync('sh', [join(here, 'install-local.sh'), ...args], { stdio: 'inherit' })
      : (console.error('app:install supports Windows and macOS; on Linux use `npm run app:build`.'), { status: 1 });
process.exit(run.status ?? 1);
