import { cleanupPlaywrightDatabase } from './playwright-fixture.js';

/** @param {import('@playwright/test').FullConfig} config */
export default function teardownPlaywrightDatabase(config) {
	const fixtureDirectory = config.metadata.playwrightFixtureDirectory;
	if (typeof fixtureDirectory === 'string') {
		cleanupPlaywrightDatabase(fixtureDirectory);
	}
}
