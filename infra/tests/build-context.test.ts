import * as BunFileSystem from '@effect/platform-bun/BunFileSystem';
import * as BunPath from '@effect/platform-bun/BunPath';
import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { hashAllowlistedBuildContext } from '../build-context.ts';

const dockerignore = ['*', '!package.json', '!apps/web', 'apps/web/node_modules', ''].join('\n');

let root: string;

const write = async (relativePath: string, content: string) => {
	await mkdir(join(root, relativePath, '..'), { recursive: true });
	await writeFile(join(root, relativePath), content);
};

const hash = (buildArgs: Record<string, string> = {}) =>
	Effect.runPromise(
		hashAllowlistedBuildContext({
			context: root,
			dockerfile: 'apps/web/Dockerfile',
			platform: 'linux/amd64',
			buildArgs
		}).pipe(Effect.provide(Layer.mergeAll(BunFileSystem.layer, BunPath.layer)))
	);

beforeEach(async () => {
	root = await mkdtemp(join(tmpdir(), 'atlantis-build-context-'));
	await write('package.json', '{}');
	await write('apps/web/Dockerfile', 'FROM scratch\n');
	await write('apps/web/Dockerfile.dockerignore', dockerignore);
	await write('apps/web/src/index.ts', 'export {};\n');
	await write('apps/web/node_modules/dep/index.js', '');
	await write('data/captures/nfcapd.202601010000', 'capture');
});

afterEach(async () => {
	await chmod(join(root, 'data'), 0o755);
	await rm(root, { recursive: true, force: true });
});

describe('hashAllowlistedBuildContext', () => {
	it('changes when an allowlisted input changes', async () => {
		const before = await hash();
		await write('apps/web/src/index.ts', 'export const changed = true;\n');
		expect(await hash()).not.toBe(before);
	});

	it('changes when a build argument changes', async () => {
		expect(await hash({ RUNTIME_UID: '1000' })).not.toBe(await hash({ RUNTIME_UID: '1001' }));
	});

	it('ignores excluded subtrees and files outside the allowlist', async () => {
		const before = await hash();
		await write('apps/web/node_modules/dep/index.js', 'changed');
		await write('data/captures/nfcapd.202601010005', 'capture');
		await write('README.md', 'changed');
		expect(await hash()).toBe(before);
	});

	it('does not walk directories outside the allowlist', async () => {
		await chmod(join(root, 'data'), 0o000);
		await expect(hash()).resolves.toMatch(/^[0-9a-f]{32}$/);
	});

	it('rejects an ignore file that is not an allowlist', async () => {
		await write('apps/web/Dockerfile.dockerignore', 'node_modules\n');
		await expect(hash()).rejects.toThrow(/must start with '\*'/);
	});
});
