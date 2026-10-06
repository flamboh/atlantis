import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { selfHostedRepoRoot, selfHostedStateDirectory } from './self-hosted-config.ts';

const [action, ...args] = process.argv.slice(2);
if (!['deploy', 'plan', 'destroy'].includes(action))
	throw new Error('Expected deploy, plan, or destroy');
const stageIndex = args.indexOf('--stage');
const stage =
	stageIndex >= 0
		? args[stageIndex + 1]
		: (args.find((arg) => arg.startsWith('--stage='))?.slice(8) ??
			process.env.ALCHEMY_STAGE ??
			'prod');
const directory = selfHostedStateDirectory(process.env.ATLANTIS_SELF_HOSTED_STATE_DIR, stage);
await mkdir(directory, { recursive: true });
process.chdir(directory);
process.argv = [
	process.argv[0],
	process.argv[1],
	action,
	resolve(selfHostedRepoRoot, 'infra/self-hosted.ts'),
	...args
];
const { main } = await import('alchemy/Cli/main');
const { runMain } = await import('alchemy/Util/PlatformServices');
main.pipe(runMain);
