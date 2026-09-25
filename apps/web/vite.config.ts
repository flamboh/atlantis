import { fileURLToPath } from 'node:url';
import adapterNode from '@sveltejs/adapter-node';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import { defaultServerConditions, type Plugin } from 'vite';
import { defineConfig } from 'vitest/config';

const repoRoot = fileURLToPath(new URL('../..', import.meta.url));

const D1_CONDITION = 'atlantis-d1';

type DatabaseDriver = 'd1' | 'sqlite';

function resolveDatabaseDriver(command: 'build' | 'serve', mode: string): DatabaseDriver {
	if (mode === 'test') {
		return 'sqlite';
	}
	const configured =
		process.env.ATLANTIS_DB_DRIVER?.trim() || (command === 'build' ? 'd1' : 'sqlite');
	if (configured !== 'd1' && configured !== 'sqlite') {
		throw new Error(`ATLANTIS_DB_DRIVER must be 'd1' or 'sqlite', received '${configured}'`);
	}
	if (configured === 'd1' && command === 'serve') {
		throw new Error(
			'The d1 driver runs only in a deployed worker. Deploy a stage with `bun run deploy:cloudflare --stage <name>`.'
		);
	}
	return configured;
}

function databaseDriver(driver: DatabaseDriver): Plugin {
	return {
		name: 'atlantis-database-driver',
		configEnvironment(name, options) {
			if (driver !== 'd1' || name === 'client' || options.consumer === 'client') {
				return;
			}
			options.resolve ??= {};
			options.resolve.conditions = [
				D1_CONDITION,
				...(options.resolve.conditions ?? defaultServerConditions)
			];
		},
		resolveId: {
			filter: { id: /^cloudflare:workers$/ },
			handler(id) {
				return driver === 'd1' ? { id, external: true } : undefined;
			}
		}
	};
}

export default defineConfig(({ command, mode }) => {
	const driver = resolveDatabaseDriver(command, mode);
	const nodeBuild = command === 'build' && driver === 'sqlite';

	return {
		plugins: [
			tailwindcss(),
			databaseDriver(driver),
			sveltekit({
				preprocess: vitePreprocess(),
				adapter: nodeBuild ? adapterNode() : undefined,
				env: {
					dir: '../..'
				}
			})
		],
		server: {
			fs: {
				allow: [repoRoot]
			}
		},
		test: {
			environment: 'node',
			include: ['tests/**/*.test.ts'],
			exclude: ['tests/e2e/**'],
			setupFiles: ['tests/setup/private-env.ts']
		}
	};
});
