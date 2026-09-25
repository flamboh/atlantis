import * as Alchemy from 'alchemy';
import * as Cloudflare from 'alchemy/Cloudflare';
import * as Docker from 'alchemy/Docker';
import { hashDockerBuildInputs } from 'alchemy/Docker/BuildHash';
import * as Config from 'effect/Config';
import * as Effect from 'effect/Effect';
import { appName, repoRoot, stageName, webDockerfile } from './shared.ts';

const campusName = `${appName}-campus`;

const webPort = 3000;

const platform = 'linux/amd64';

const parseDataUser = (value: string) => {
	const match = /^(\d+):(\d+)$/.exec(value.trim());
	if (!match) {
		throw new Error(`ATLANTIS_CAMPUS_DATA_USER must be '<uid>:<gid>', got '${value}'`);
	}
	return { uid: match[1], gid: match[2] };
};

const webHealthcheck = `node -e "fetch('http://127.0.0.1:${webPort}/api/datasets').then((response) => process.exit(response.ok ? 0 : 1), () => process.exit(1))"`;

export default Alchemy.Stack(
	campusName,
	{
		providers: Docker.providers(),
		state: Cloudflare.state()
	},
	Effect.gen(function* () {
		const { stage } = yield* Alchemy.Stack;

		const dockerHost = yield* Config.String('ATLANTIS_CAMPUS_DOCKER_HOST');
		const dataDir = yield* Config.String('ATLANTIS_CAMPUS_DATA_DIR');
		const port = yield* Config.Port('ATLANTIS_CAMPUS_PORT').pipe(Config.withDefault(8080));
		const dataUser = yield* Config.String('ATLANTIS_CAMPUS_DATA_USER').pipe(
			Config.withDefault('1000:1000'),
			Config.map(parseDataUser)
		);

		if (!dataDir.startsWith('/') || dataDir === '/') {
			return yield* Effect.die(
				new Error(`ATLANTIS_CAMPUS_DATA_DIR must be an absolute host directory, got '${dataDir}'`)
			);
		}

		const webName = stageName(`${campusName}-web`, stage);
		const buildArgs = { RUNTIME_UID: dataUser.uid, RUNTIME_GID: dataUser.gid };
		const buildHash = yield* hashDockerBuildInputs(
			{ context: repoRoot, dockerfile: webDockerfile, platform, buildArgs },
			'effective'
		).pipe(Effect.orDie);

		const context = yield* Docker.Context('DockerHost', {
			name: stageName(campusName, stage),
			docker: `host=${dockerHost}`,
			description: `ATLANTIS campus deployment (${stage})`
		});

		const webImage = yield* Docker.Image('WebImage', {
			name: webName,
			tag: buildHash,
			context,
			build: {
				context: repoRoot,
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
			volumes: [{ hostPath: dataDir, containerPath: '/data' }],
			ports: [{ external: `127.0.0.1:${port}`, internal: webPort }],
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
			tunnel: `ssh -N -L ${port}:127.0.0.1:${port} ${dockerHost.replace(/^ssh:\/\//, '')}`
		};
	})
);
