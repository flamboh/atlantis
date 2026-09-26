export const appName = 'atlantis';

export const productionStage = 'prod';

export const webRoot = 'apps/web';

export const webMigrationsDir = `${webRoot}/drizzle`;

const stagePattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

export const invalidStageMessage = (stage: string): string | undefined =>
	stagePattern.test(stage)
		? undefined
		: `Invalid stage '${stage}'. Use lowercase letters, digits, and single hyphens, for example '${stage
				.toLowerCase()
				.replace(/[^a-z0-9]+/g, '-')
				.replace(/^-|-$/g, '')}'.`;

export const isProduction = (stage: string) => stage === productionStage;

export const stageName = (base: string, stage: string) =>
	isProduction(stage) ? base : `${base}-${stage}`;
