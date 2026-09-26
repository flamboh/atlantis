import { describe, expect, it } from 'vitest';
import { invalidStageMessage, stageName } from '../shared.ts';

describe('invalidStageMessage', () => {
	it.each(['prod', 'alice', 'alice-dev', 'pr-116', 'a1-b2-c3'])('accepts %s', (stage) => {
		expect(invalidStageMessage(stage)).toBeUndefined();
	});

	it.each([
		['alice_dev', 'alice-dev'],
		['live_Alice', 'live-alice'],
		['Alice', 'alice'],
		['alice--dev', 'alice-dev'],
		['-alice', 'alice'],
		['alice-', 'alice'],
		['alice.dev', 'alice-dev']
	])('rejects %s and suggests %s', (stage, suggestion) => {
		expect(invalidStageMessage(stage)).toContain(`'${suggestion}'`);
	});

	it('rejects an empty stage', () => {
		expect(invalidStageMessage('')).toBeDefined();
	});
});

describe('stageName', () => {
	it('keeps the base name for production', () => {
		expect(stageName('atlantis-db', 'prod')).toBe('atlantis-db');
	});

	it('suffixes other stages unchanged', () => {
		expect(stageName('atlantis-db', 'alice-dev')).toBe('atlantis-db-alice-dev');
	});

	it('gives every accepted stage a distinct name', () => {
		const stages = ['alice-dev', 'alice', 'dev', 'alice-dev-2'];
		const names = stages.map((stage) => stageName('atlantis', stage));
		expect(new Set(names).size).toBe(stages.length);
	});
});
