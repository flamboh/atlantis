export interface SshDockerHost {
	url: string;
	user: string | undefined;
	host: string;
	port: string | undefined;
}

export const parseSshDockerHost = (value: string): SshDockerHost => {
	const url = URL.parse(value.trim());
	if (url === null || url.protocol !== 'ssh:' || url.hostname === '') {
		throw new Error(`ATLANTIS_SELF_HOSTED_DOCKER_HOST must be an ssh:// URL, got '${value}'`);
	}
	return {
		url: value.trim(),
		user: url.username === '' ? undefined : decodeURIComponent(url.username),
		host: url.hostname.replace(/^\[(.*)\]$/, '$1'),
		port: url.port === '' ? undefined : url.port
	};
};

const shellQuote = (value: string) =>
	/^[\w@%+=:,./-]+$/.test(value) ? value : `'${value.replaceAll("'", `'\\''`)}'`;

export const sshTunnelCommand = (dockerHost: SshDockerHost, port: number) => {
	const destination =
		dockerHost.user === undefined ? dockerHost.host : `${dockerHost.user}@${dockerHost.host}`;
	const portArgs = dockerHost.port === undefined ? [] : ['-p', dockerHost.port];
	return ['ssh', '-N', '-L', `${port}:127.0.0.1:${port}`, ...portArgs, destination]
		.map(shellQuote)
		.join(' ');
};
