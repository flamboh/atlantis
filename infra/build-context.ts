import { selectDockerBuildContext, type DockerBuildSource } from 'alchemy/Docker/BuildHash';
import * as Effect from 'effect/Effect';
import * as FileSystem from 'effect/FileSystem';
import * as Path from 'effect/Path';
import * as Result from 'effect/Result';
import * as Stream from 'effect/Stream';
import * as crypto from 'node:crypto';

interface ContextEntry {
	path: string;
	fullPath: string;
	type: string;
	mode?: number;
	size?: string;
	target?: string;
}

const allowlistedRoots = (dockerignore: string, ignoreFile: string) => {
	const rules = dockerignore
		.replace(/^\uFEFF/, '')
		.split(/\r?\n/)
		.map((line) => line.trim())
		.filter((line) => line.length > 0 && !line.startsWith('#'));
	if (rules[0] !== '*') {
		return Effect.fail(
			new Error(`${ignoreFile} must start with '*' and allowlist the build inputs`)
		);
	}
	return Effect.succeed(
		rules
			.filter((rule) => rule.startsWith('!'))
			.map((rule) =>
				rule
					.slice(1)
					.trim()
					.replace(/^\.?\/+/, '')
					.replace(/\/+$/, '')
			)
	);
};

export const hashAllowlistedBuildContext = Effect.fn(function* (source: DockerBuildSource) {
	const fs = yield* FileSystem.FileSystem;
	const path = yield* Path.Path;
	const selection = yield* selectDockerBuildContext(source);
	const ignoreFile = `${selection.dockerfile}.dockerignore`;
	const ignoreContent = yield* fs.readFileString(ignoreFile);
	const roots = yield* allowlistedRoots(ignoreContent, ignoreFile);

	const entries: ContextEntry[] = [];
	const pending = [...roots];
	while (pending.length > 0) {
		const relativePath = pending.pop()!;
		if (!selection.includes(relativePath)) {
			continue;
		}
		const fullPath = path.join(selection.context, relativePath);
		const link = yield* Effect.result(fs.readLink(fullPath));
		if (Result.isSuccess(link)) {
			entries.push({ path: relativePath, fullPath, type: 'SymbolicLink', target: link.success });
			continue;
		}
		const info = yield* fs.stat(fullPath);
		entries.push({
			path: relativePath,
			fullPath,
			type: info.type,
			mode: info.mode & 0o7777,
			size: info.type === 'File' ? String(info.size) : undefined
		});
		if (info.type === 'Directory') {
			for (const child of yield* fs.readDirectory(fullPath)) {
				pending.push(`${relativePath}/${child}`);
			}
		}
	}

	const hasher = crypto.createHash('sha256');
	hasher.update(
		JSON.stringify({
			platform: source.platform,
			buildArgs: Object.entries(source.buildArgs ?? {}).sort(([a], [b]) =>
				a < b ? -1 : a > b ? 1 : 0
			),
			dockerignore: ignoreContent
		})
	);
	hasher.update('Dockerfile\0');
	hasher.update(yield* fs.readFile(selection.dockerfile));

	for (const { fullPath, ...entry } of entries.sort((a, b) =>
		a.path < b.path ? -1 : a.path > b.path ? 1 : 0
	)) {
		hasher.update(`${JSON.stringify(entry)}\0`);
		if (entry.type === 'File') {
			yield* fs
				.stream(fullPath)
				.pipe(Stream.runForEach((chunk) => Effect.sync(() => hasher.update(chunk))));
		}
	}

	return hasher.digest('hex').slice(0, 32);
});
