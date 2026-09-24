import { defineEnvVars } from '@sveltejs/kit/env';

const optional = (value: string | undefined) => value?.trim() || undefined;

export const variables = defineEnvVars({
	LOCAL_SQLITE_PATH: {
		description: 'Path to a single pipeline SQLite product; disables data/ discovery when set',
		schema: optional
	},
	DATABASE_PATH: {
		description: 'Fallback for LOCAL_SQLITE_PATH',
		schema: optional
	},
	DEFAULT_DATASET: {
		description: 'Dataset ID selected when a request does not name one',
		schema: optional
	}
});
