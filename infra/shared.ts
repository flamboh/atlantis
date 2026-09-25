export const appName = 'atlantis';

export const productionStage = 'prod';

export const webRoot = 'apps/web';

export const webMigrationsDir = `${webRoot}/drizzle`;

export const isProduction = (stage: string) => stage === productionStage;

export const stageName = (base: string, stage: string) =>
	isProduction(stage) ? base : `${base}-${stage.replaceAll('_', '-')}`;

export const repoRoot = '.';

export const webDockerfile = `${webRoot}/Dockerfile`;
