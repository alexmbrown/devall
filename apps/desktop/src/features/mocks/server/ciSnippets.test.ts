import { describe, expect, it } from 'vitest';
import { cliCommands, compose, dockerCommands, githubActions, type CiOptions } from './ciSnippets';

const base: CiOptions = { image: 'devall-mock', port: 8080, chaos: '', projectDir: '.' };

describe('Run in CI snippets', () => {
  it('docker run mounts the project read-only and appends chaos', () => {
    const d = dockerCommands({ ...base, chaos: 'flaky', projectDir: 'services/api/' });
    expect(d.bash.split('\n')[0]).toBe('docker run --rm -p 8080:8080 -v "$PWD/services/api:/project:ro" devall-mock --chaos flaky');
    expect(d.powershell.split('\n')[0]).toBe('docker run --rm -p 8080:8080 -v "${PWD}\\services\\api:/project:ro" devall-mock --chaos flaky');
    expect(dockerCommands(base).bash).toContain('--entrypoint devall devall-mock mock check --project /project');
  });

  it('compose and GitHub Actions use the same port and mount', () => {
    const c = compose({ ...base, port: 9090, chaos: 'slow', projectDir: './api' });
    expect(c).toContain('- "9090:8080"');
    expect(c).toContain('- ./api:/project:ro');
    expect(c).toContain('command: ["--chaos", "slow"]');
    expect(compose(base)).not.toContain('command:');
    const gh = githubActions({ ...base, projectDir: 'api' });
    expect(gh).toContain('-v "$GITHUB_WORKSPACE/api:/project:ro" devall-mock');
    expect(gh).toContain('http://localhost:8080/__admin/health');
  });

  it('CLI commands include the load baseline when a test is given', () => {
    expect(cliCommands(base)).not.toContain('--against-mock');
    expect(cliCommands({ ...base, loadTest: 'smoke.load.yaml', chaos: 'all' })).toContain('devall load run smoke.load.yaml --against-mock');
    expect(cliCommands({ ...base, chaos: 'all' })).toContain('--bind 0.0.0.0 --chaos all');
  });
});
