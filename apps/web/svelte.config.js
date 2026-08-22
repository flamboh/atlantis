import adapter from '@sveltejs/adapter-cloudflare';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

// The adapter's dev-time platform emulation boots workerd, which local SQLite
// development never needs. Emulate only when explicitly developing against D1.
const cloudflare = adapter();
if (process.env.ATLANTIS_DB_DRIVER !== 'd1') {
	delete cloudflare.emulate;
}

/** @type {import('@sveltejs/kit').Config} */
const config = {
	// Consult https://svelte.dev/docs/kit/integrations
	// for more information about preprocessors
	preprocess: vitePreprocess(),

	kit: {
		adapter: cloudflare,
		env: {
			dir: '../..' // Look for .env files in repository root
		}
	}
};

export default config;
