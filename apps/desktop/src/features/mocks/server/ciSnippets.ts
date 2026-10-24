/**
 * Copy-paste commands for running a project's mocks outside the app: the `devall` CLI, the
 * Docker image (Dockerfile at the /dev/all repo root), GitHub Actions and docker-compose.
 * See docs/MOCKS_IN_CI.md.
 */

export interface CiOptions {
  /** Image name, e.g. `devall-mock` (local build) or `ghcr.io/<owner>/devall-mock:1`. */
  image: string;
  port: number;
  /** Chaos preset (`slow`, `flaky`, `broken`, `all`) or empty. */
  chaos: string;
  /** The project folder relative to the repository root (`.` when it is the root). */
  projectDir: string;
  /** A load test path under `load/`, for the mock-baseline command. */
  loadTest?: string;
}

const clean = (dir: string) => {
  const d = dir.trim().replace(/\\/g, '/').replace(/^\.\/?/, '').replace(/\/+$/, '');
  return d;
};

/** `./` or `./services/api` style relative path for compose / docker. */
function rel(dir: string): string {
  const d = clean(dir);
  return d ? `./${d}` : '.';
}

function extraArgs(o: CiOptions): string[] {
  return o.chaos ? ['--chaos', o.chaos] : [];
}

export function cliCommands(o: CiOptions): string {
  const chaos = o.chaos ? ` --chaos ${o.chaos}` : '';
  const lines = [
    '# check that every stub and data source loads (exit 1 otherwise)',
    'devall mock check',
    '',
    '# serve mocks/ on all interfaces until Ctrl+C (request log on stdout)',
    `devall mock serve --port ${o.port} --bind 0.0.0.0${chaos}`,
    '',
    '# lint the API spec / check stubs against it',
    'devall openapi lint openapi/api.yaml',
    'devall openapi drift --spec openapi/api.yaml --mocks',
  ];
  if (o.loadTest) {
    lines.push('', '# load-test baseline against the mock (client/tooling overhead)', `devall load run ${o.loadTest} --against-mock`);
  }
  return lines.join('\n');
}

export function dockerCommands(o: CiOptions): { bash: string; powershell: string } {
  const args = extraArgs(o).join(' ');
  const tail = args ? ` ${args}` : '';
  const d = clean(o.projectDir);
  const sub = d ? `/${d}` : '';
  return {
    bash: [
      `docker run --rm -p ${o.port}:8080 -v "$PWD${sub}:/project:ro" ${o.image}${tail}`,
      '',
      '# check only (non-zero exit on load errors)',
      `docker run --rm -v "$PWD${sub}:/project:ro" --entrypoint devall ${o.image} mock check --project /project`,
    ].join('\n'),
    powershell: [
      `docker run --rm -p ${o.port}:8080 -v "\${PWD}${sub.replace(/\//g, '\\')}:/project:ro" ${o.image}${tail}`,
      '',
      '# check only (non-zero exit on load errors)',
      `docker run --rm -v "\${PWD}${sub.replace(/\//g, '\\')}:/project:ro" --entrypoint devall ${o.image} mock check --project /project`,
    ].join('\n'),
  };
}

export function githubActions(o: CiOptions): string {
  const d = clean(o.projectDir);
  const mount = d ? `$GITHUB_WORKSPACE/${d}` : '$GITHUB_WORKSPACE';
  const args = extraArgs(o).join(' ');
  return `jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # Service containers start before checkout, so the mock runs as a step with the repo mounted.
      - name: Start the mock server
        run: docker run -d --name mocks -p ${o.port}:8080 -v "${mount}:/project:ro" ${o.image}${args ? ` ${args}` : ''}
      - name: Wait until it is healthy
        run: for i in $(seq 1 30); do curl -fsS http://localhost:${o.port}/__admin/health && break; sleep 1; done
      - name: Tests against the mocks
        run: npm test
        env:
          API_BASE_URL: http://localhost:${o.port}
      - name: Mock request log
        if: always()
        run: docker logs mocks`;
}

export function compose(o: CiOptions): string {
  const args = extraArgs(o);
  const command = args.length ? `\n    command: [${args.map((a) => JSON.stringify(a)).join(', ')}]` : '';
  return `services:
  mocks:
    image: ${o.image}
    ports:
      - "${o.port}:8080"
    volumes:
      - ${rel(o.projectDir)}:/project:ro${command}
    healthcheck:
      test: ["CMD", "devall", "mock", "health", "--port", "8080"]
      interval: 10s`;
}
