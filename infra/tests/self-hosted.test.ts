import { describe, expect, it } from 'vitest';
import {
	selfHostedContainerSettings,
	selfHostedRepoRoot,
	selfHostedStateDirectory
} from '../self-hosted-config.ts';

describe('self-hosted deployment isolation', () => {
	it('preserves the test stage port, name, and writable mount', () => {
		expect(selfHostedContainerSettings('test', '/srv/data', 8080, false)).toEqual({
			name: 'atlantis-self-hosted-web-test',
			volumes: [{ hostPath: '/srv/data', containerPath: '/data', readOnly: false }],
			ports: [{ external: '127.0.0.1:8080', internal: 3000 }]
		});
	});
	it('namespaces perf and mounts its data read-only on loopback', () => {
		expect(selfHostedContainerSettings('perf', '/srv/data', 8090, true)).toEqual({
			name: 'atlantis-perf-web',
			volumes: [{ hostPath: '/srv/data', containerPath: '/data', readOnly: true }],
			ports: [{ external: '127.0.0.1:8090', internal: 3000 }]
		});
	});
	it.each([
		[8080, true],
		[8090, false]
	])('rejects unsafe perf settings', (port, readOnly) => {
		expect(() => selfHostedContainerSettings('perf', '/srv/data', port, readOnly)).toThrow();
	});
	it.each(['relative', '/', '/srv/..'])('rejects invalid data directory %s', (directory) => {
		expect(() => selfHostedContainerSettings('perf', directory, 8090, true)).toThrow();
	});
	it('requires external perf state while preserving the normal checkout default', () => {
		expect(selfHostedStateDirectory(undefined, 'test')).toBe(selfHostedRepoRoot);
		expect(selfHostedStateDirectory('/srv/perf', 'perf')).toBe('/srv/perf');
		for (const directory of [
			undefined,
			'relative',
			'/',
			selfHostedRepoRoot,
			`${selfHostedRepoRoot}/state`
		]) {
			expect(() => selfHostedStateDirectory(directory, 'perf')).toThrow();
		}
	});
});
