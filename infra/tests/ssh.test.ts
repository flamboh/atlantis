import { describe, expect, it } from 'vitest';
import { parseSshDockerHost, sshTunnelCommand } from '../ssh.ts';

describe('sshTunnelCommand', () => {
	it('uses an SSH config alias as the destination', () => {
		expect(sshTunnelCommand(parseSshDockerHost('ssh://docker-host'), 8080)).toBe(
			'ssh -N -L 8080:127.0.0.1:8080 docker-host'
		);
	});

	it('passes a non-default SSH port with -p', () => {
		expect(sshTunnelCommand(parseSshDockerHost('ssh://user@host.example.com:2222'), 8081)).toBe(
			'ssh -N -L 8081:127.0.0.1:8081 -p 2222 user@host.example.com'
		);
	});

	it('unwraps bracketed IPv6 hosts', () => {
		expect(sshTunnelCommand(parseSshDockerHost('ssh://user@[2001:db8::1]:2222'), 8080)).toBe(
			'ssh -N -L 8080:127.0.0.1:8080 -p 2222 user@2001:db8::1'
		);
	});

	it('quotes arguments that the shell would reinterpret', () => {
		expect(sshTunnelCommand(parseSshDockerHost("ssh://o'brien%20x@host.example.com"), 8080)).toBe(
			`ssh -N -L 8080:127.0.0.1:8080 'o'\\''brien x@host.example.com'`
		);
	});
});

describe('parseSshDockerHost', () => {
	it('keeps the configured URL for the Docker context', () => {
		expect(parseSshDockerHost(' ssh://user@host.example.com:2222 ').url).toBe(
			'ssh://user@host.example.com:2222'
		);
	});

	it.each(['host.example.com', 'tcp://host.example.com:2375', 'ssh://'])('rejects %s', (value) => {
		expect(() => parseSshDockerHost(value)).toThrow(/must be an ssh:\/\/ URL/);
	});
});
