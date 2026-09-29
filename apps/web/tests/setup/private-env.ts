import { vi } from 'vitest';

vi.mock('$app/env/private', async () => {
	const { variables } = await import('../../src/env.ts');
	const env = {};
	for (const name of Object.keys(variables)) {
		Object.defineProperty(env, name, {
			enumerable: true,
			get: () => process.env[name]?.trim() || undefined
		});
	}
	return env;
});
