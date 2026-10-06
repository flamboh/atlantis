import * as Alchemy from 'alchemy';
import * as Docker from 'alchemy/Docker';
import * as Config from 'effect/Config';
import * as ConfigProvider from 'effect/ConfigProvider';
import * as Effect from 'effect/Effect';
import { hashAllowlistedBuildContext } from './build-context.ts';
import { appName, invalidStageMessage, stageName, webDockerfile } from './shared.ts';
import {
	selfHostedContainerSettings,
	selfHostedRepoRoot,
	selfHostedStateDirectory
} from './self-hosted-config.ts';
import { parseSshDockerHost, sshTunnelCommand } from './ssh.ts';

const selfHostedName = `${appName}-self-hosted`;

const webPort = 3000;

const platform = 'linux/amd64';

const parseDataUser = (value: string) => {
	const match = /^(\d+):(\d+)$/.exec(value.trim());
	if (!match) {
		throw new Error(`ATLANTIS_SELF_HOSTED_DATA_USER must be '<uid>:<gid>', got '${value}'`);
	}
	return { uid: match[1], gid: match[2] };
};

const webHealthcheck = `node -e "fetch('http://127.0.0.1:${webPort}/api/datasets').then((response) => process.exit(response.ok ? 0 : 1), () => process.exit(1))"`;

export default Alchemy.Stack(
	selfHostedName,
	{
		providers: Docker.providers(),
		state: Alchemy.localState()
	},
	Effect.gen(function* () {
		const { stage } = yield* Alchemy.Stack;
		const invalidStage = invalidStageMessage(stage);
		if (invalidStage) {
			return yield* Effect.fail(
				new Config.ConfigError(new ConfigProvider.SourceError({ message: invalidStage }))
			);
		}

		const dockerHost = yield* Config.String('ATLANTIS_SELF_HOSTED_DOCKER_HOST').pipe(
			Config.map(parseSshDockerHost)
		);
		const dataDir = yield* Config.String('ATLANTIS_SELF_HOSTED_DATA_DIR');
		const port = yield* Config.Port('ATLANTIS_SELF_HOSTED_PORT').pipe(
			Config.withDefault(stage === 'perf' ? 8090 : 8080)
		);
		const readOnly = yield* Config.Boolean('ATLANTIS_SELF_HOSTED_READ_ONLY').pipe(
			Config.withDefault(stage === 'perf')
		);
		const dataUser = yield* Config.String('ATLANTIS_SELF_HOSTED_DATA_USER').pipe(
			Config.withDefault('1000:1000'),
			Config.map(parseDataUser)
		);

		if (stage === 'perf') {
			const stateDirectory = selfHostedStateDirectory(
				process.env.ATLANTIS_SELF_HOSTED_STATE_DIR,
				stage
			);
			if (process.cwd() !== stateDirectory)
				return yield* Effect.die(
					new Error(
						'Deploy perf through bun run deploy:self-hosted so Alchemy uses the external state directory'
					)
				);
		}

		const settings = selfHostedContainerSettings(stage, dataDir, port, readOnly);
		const webName = settings.name;
		const buildArgs = { RUNTIME_UID: dataUser.uid, RUNTIME_GID: dataUser.gid };
		const buildHash = yield* hashAllowlistedBuildContext({
			context: selfHostedRepoRoot,
			dockerfile: webDockerfile,
			platform,
			buildArgs
		}).pipe(Effect.orDie);

		const context = yield* Docker.Context('DockerHost', {
			name: stage === 'perf' ? `${appName}-perf` : stageName(selfHostedName, stage),
			docker: `host=${dockerHost.url}`,
			description: `ATLANTIS self-hosted deployment (${stage})`
		});

		const webImage = yield* Docker.Image('WebImage', {
			name: webName,
			tag: buildHash,
			context,
			build: {
				context: selfHostedRepoRoot,
				dockerfile: webDockerfile,
				platform,
				args: buildArgs
			}
		});

		const web = yield* Docker.Container('Web', {
			name: webName,
			context,
			image: `${webName}:${buildHash}`,
			environment: {
				NODE_OPTIONS: '--max-old-space-size=768'
			},
			volumes: settings.volumes,
			ports: settings.ports,
			memory: '1g',
			restart: 'unless-stopped',
			stopTimeout: '30 seconds',
			healthcheck: {
				cmd: webHealthcheck,
				interval: '30 seconds',
				timeout: '10 seconds',
				retries: 3,
				startPeriod: '30 seconds'
			},
			start: true
		});
		yield* web.bind('WebImage', webImage.imageId as never);

		return {
			container: web.name,
			image: webImage.imageRef,
			dataDir,
			tunnel: sshTunnelCommand(dockerHost, port)
		};
	})
);
