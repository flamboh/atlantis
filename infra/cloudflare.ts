import * as Alchemy from 'alchemy';
import * as Cloudflare from 'alchemy/Cloudflare';
import * as Config from 'effect/Config';
import * as ConfigProvider from 'effect/ConfigProvider';
import * as Effect from 'effect/Effect';
import {
	appName,
	invalidStageMessage,
	isProduction,
	stageName,
	webMigrationsDir,
	webRoot
} from './shared.ts';

export default Alchemy.Stack(
	appName,
	{
		providers: Cloudflare.providers(),
		state: Cloudflare.state()
	},
	Effect.gen(function* () {
		const { stage } = yield* Alchemy.Stack;
		const invalidStage = invalidStageMessage(stage);
		if (invalidStage) {
			return yield* Effect.fail(
				new Config.ConfigError(new ConfigProvider.SourceError({ message: invalidStage }))
			);
		}
		const production = isProduction(stage);

		const database = yield* Cloudflare.D1.Database('Database', {
			name: stageName(`${appName}-db`, stage),
			migrations: webMigrationsDir
		}).pipe(Alchemy.RemovalPolicy.retain(production));

		const dashboard = yield* Cloudflare.Website.SvelteKit('Dashboard', {
			name: stageName(appName, stage),
			rootDir: webRoot,
			compatibility: {
				date: '2026-05-10',
				flags: ['nodejs_compat']
			},
			observability: {
				enabled: true
			},
			memo: {
				include: [
					'src/**',
					'static/**',
					'drizzle/**',
					'package.json',
					'tsconfig.json',
					'vite.config.ts'
				],
				lockfile: true
			},
			env: {
				DB: database
			}
		}).pipe(Alchemy.RemovalPolicy.retain(production), Alchemy.AdoptPolicy.adopt(production));

		return {
			url: dashboard.url,
			databaseName: database.databaseName
		};
	})
);
