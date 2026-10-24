export interface StartupOpts {
  port?: number;
  cors?: boolean;
  templating?: boolean;
  verbose?: boolean;
  basicAuth?: string;
  proxyAll?: string;
  browserProxying?: boolean;
  httpsPort?: number;
  rootDir?: string;
  journalMax?: number;
  version?: string;
}

/** Ready-to-paste commands for the common ways of running WireMock. */
export function startupCommands(o: StartupOpts = {}): { label: string; cmd: string }[] {
  const port = o.port ?? 8080;
  const version = o.version ?? '3.13.2';
  const flags: string[] = [];
  if (o.cors !== false) flags.push('--enable-stub-cors');
  if (o.templating) flags.push('--global-response-templating');
  if (o.verbose) flags.push('--verbose');
  if (o.basicAuth) flags.push(`--admin-api-basic-auth ${o.basicAuth}`);
  if (o.proxyAll) flags.push(`--proxy-all ${o.proxyAll}`);
  if (o.browserProxying) flags.push('--enable-browser-proxying');
  if (o.httpsPort) flags.push(`--https-port ${o.httpsPort}`);
  if (o.journalMax) flags.push(`--max-request-journal-entries ${o.journalMax}`);
  const f = flags.join(' ');
  const dir = (o.rootDir ?? '').replace(/^\.\//, '').replace(/\/+$/, '');
  const vol = dir ? `$PWD/${dir}` : '$PWD';
  return [
    {
      label: 'Docker',
      cmd: `docker run -it --rm -p ${port}:8080${o.httpsPort ? ` -p ${o.httpsPort}:${o.httpsPort}` : ''} -v "${vol}:/home/wiremock" wiremock/wiremock:${version} ${f}`.trim(),
    },
    {
      label: 'Java (standalone jar)',
      cmd: `java -jar wiremock-standalone-${version}.jar --port ${port}${o.rootDir ? ` --root-dir ${o.rootDir}` : ''} ${f}`.trim(),
    },
    { label: 'npm (bundled jar, needs Java 11+)', cmd: `npx wiremock --port ${port}${o.rootDir ? ` --root-dir ${o.rootDir}` : ''} ${f}`.trim() },
    {
      label: 'docker-compose',
      cmd: `services:
  wiremock:
    image: wiremock/wiremock:${version}
    ports: ["${port}:8080"]
    volumes: ["./${dir || 'wiremock'}:/home/wiremock"]
    command: ${JSON.stringify(f)}`,
    },
  ];
}
