import fs from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
	cleanupPlaywrightDatabase,
	seedPlaywrightDatabase
} from '../../tests/e2e/playwright-fixture.js';

describe('Playwright database fixture', () => {
	it('removes its owned temporary directory during teardown', () => {
		const fixture = seedPlaywrightDatabase();
		expect(fs.existsSync(fixture.databasePath)).toBe(true);

		cleanupPlaywrightDatabase(fixture.fixtureDirectory);

		expect(fs.existsSync(fixture.fixtureDirectory)).toBe(false);
	});
});
