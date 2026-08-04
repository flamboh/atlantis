import { defineConfig, devices } from '@playwright/test';
import {
	cleanupPlaywrightDatabase,
	seedPlaywrightDatabase
} from './tests/e2e/playwright-fixture.js';

const baseURL = process.env.PLAYWRIGHT_BASE_URL || 'http://127.0.0.1:4173';
const shouldManageServer = process.env.PLAYWRIGHT_WEB_SERVER === '1';
const playwrightFixture = shouldManageServer ? seedPlaywrightDatabase() : undefined;
if (playwrightFixture) {
	process.once('exit', () => cleanupPlaywrightDatabase(playwrightFixture.fixtureDirectory));
}

export default defineConfig({
	testDir: './tests/e2e',
	globalTeardown: shouldManageServer ? './tests/e2e/playwright-teardown.js' : undefined,
	metadata: playwrightFixture
		? { playwrightFixtureDirectory: playwrightFixture.fixtureDirectory }
		: {},
	timeout: 30_000,
	expect: {
		timeout: 5_000
	},
	fullyParallel: true,
	forbidOnly: Boolean(process.env.CI),
	retries: process.env.CI ? 2 : 0,
	reporter: process.env.CI ? [['html', { open: 'never' }], ['list']] : 'list',
	use: {
		baseURL,
		trace: 'on-first-retry'
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'] }
		}
	],
	webServer: shouldManageServer
		? {
				command: 'bun run preview --host 127.0.0.1 --port 4173',
				env: {
					...process.env,
					ATLANTIS_DB_DRIVER: 'sqlite',
					LOCAL_SQLITE_PATH: playwrightFixture?.databasePath
				},
				port: 4173,
				reuseExistingServer: false,
				timeout: 120_000
			}
		: undefined
});
