import { isAbsolute, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { appName, stageName } from './shared.ts';

export const selfHostedRepoRoot = fileURLToPath(new URL('../', import.meta.url));

export function selfHostedStateDirectory(value: string | undefined, stage: string) {
	if (!value) {
		if (stage === 'perf') throw new Error('The perf stage requires ATLANTIS_SELF_HOSTED_STATE_DIR');
		return selfHostedRepoRoot;
	}
	if (!isAbsolute(value) || resolve(value) === '/') {
		throw new Error('ATLANTIS_SELF_HOSTED_STATE_DIR must be an absolute non-root directory');
	}
	const directory = resolve(value);
	const fromRepo = relative(selfHostedRepoRoot, directory);
	if (
		stage === 'perf' &&
		(fromRepo === '' || (!fromRepo.startsWith('../') && !isAbsolute(fromRepo)))
	) {
		throw new Error('The perf stage state must be outside the repository');
	}
	return directory;
}

export function selfHostedContainerSettings(
	stage: string,
	dataDir: string,
	port: number,
	readOnly: boolean
) {
	if (!isAbsolute(dataDir) || resolve(dataDir) === '/') {
		throw new Error('ATLANTIS_SELF_HOSTED_DATA_DIR must be an absolute non-root directory');
	}
	if (!Number.isInteger(port) || port < 1 || port > 65535) {
		throw new Error('ATLANTIS_SELF_HOSTED_PORT must be a valid TCP port');
	}
	if (stage === 'perf' && (!readOnly || port === 8080)) {
		throw new Error('The perf stage requires read-only data and a port other than 8080');
	}
	return {
		name: stage === 'perf' ? `${appName}-perf-web` : stageName(`${appName}-self-hosted-web`, stage),
		volumes: [{ hostPath: dataDir, containerPath: '/data', readOnly }],
		ports: [{ external: `127.0.0.1:${port}`, internal: 3000 }]
	};
}
