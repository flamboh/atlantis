import { fileURLToPath } from 'node:url';
import cloudflare from '@sveltejs/adapter-cloudflare';
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
		}
	};
}

export default defineConfig(({ command, mode }) => {
	const driver = resolveDatabaseDriver(command, mode);

	return {
		plugins: [
			tailwindcss(),
			databaseDriver(driver),
			sveltekit({
				preprocess: vitePreprocess(),
				adapter: driver === 'd1' ? cloudflare() : undefined,
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
