import {
	FLOW_VISIBILITIES,
	IP_GRANULARITIES,
	type FlowVisibility,
	type IpGranularity
} from '$lib/types/types';
import type { SourceDefinition } from '$lib/server/datasets';
import type { StructureFunctionPoint } from '$lib/types/types';
type RawStructureFunctionPoint = {
	q: number;
	tau?: number;
	tauTilde?: number;
	sd?: number;
	s?: number;
};

export interface AggregateStatsParams {
	routers: string[];
	granularity: IpGranularity;
	start: number;
	end: number;
	srcVisibility: FlowVisibility;
	dstVisibility: FlowVisibility;
}

export interface RequestValidationError {
	error: string;
	status: 400;
}

export const FIVE_MINUTE_GRANULARITY: IpGranularity = '5m';
export const DEFAULT_IP_GRANULARITY: IpGranularity = '1h';
export type NetflowSchemaVersion = 'v3';

const VALID_IP_GRANULARITIES = new Set<string>(IP_GRANULARITIES);
const VALID_FLOW_VISIBILITIES = new Set<string>(FLOW_VISIBILITIES);

export function assertNetflowV3Database(): void {
	return;
}

export function getNetflowSchemaVersion(): NetflowSchemaVersion {
	return 'v3';
}

export function parseSourceIds(param: string | null): string[] {
	if (!param) return [];
	return param
		.split(',')
		.map((sourceId) => sourceId.trim())
		.filter((sourceId) => sourceId.length > 0);
}

export function parseTimestamp(param: string | null): number | null {
	if (!param) return null;
	const value = Number(param);
	return Number.isFinite(value) ? value : null;
}

export function parseIpGranularity(param: string | null): IpGranularity | null {
	if (!param) {
		return null;
	}

	return VALID_IP_GRANULARITIES.has(param) ? (param as IpGranularity) : null;
}

export function parseIpGranularityOrDefault(param: string | null): IpGranularity {
	return parseIpGranularity(param) ?? DEFAULT_IP_GRANULARITY;
}

export function parseFlowVisibility(param: string | null): FlowVisibility {
	if (!param) {
		return 'all';
	}

	return VALID_FLOW_VISIBILITIES.has(param) ? (param as FlowVisibility) : 'all';
}

export function parseAggregateStatsParams(url: URL): AggregateStatsParams | RequestValidationError {
	const routers = parseSourceIds(url.searchParams.get('routers'));
	const granularity = parseIpGranularityOrDefault(url.searchParams.get('granularity'));
	const start = parseTimestamp(url.searchParams.get('startDate'));
	const end = parseTimestamp(url.searchParams.get('endDate'));
	const srcVisibility = parseFlowVisibility(url.searchParams.get('srcVisibility'));
	const dstVisibility = parseFlowVisibility(url.searchParams.get('dstVisibility'));

	if (routers.length === 0) {
		return { error: 'No routers selected', status: 400 };
	}

	if (start === null || end === null) {
		return { error: 'Invalid start or end time', status: 400 };
	}

	if (start >= end) {
		return { error: 'Start time must be before end time', status: 400 };
	}

	return { routers, granularity, start, end, srcVisibility, dstVisibility };
}

export function placeholders(values: unknown[]): string {
	return values.map(() => '?').join(',');
}

export function resolveSourceIds(
	definitions: SourceDefinition[],
	requestedSourceIds: string[]
): string[] {
	const requested = uniqueSorted(requestedSourceIds);
	const completeDefinitions = ensureRequestedSourceDefinitions(definitions, requested);
	const targetMembers = expandRequestedMembers(completeDefinitions, requested);
	const exactSource = findExactSourceForMembers(completeDefinitions, targetMembers, requested);
	if (exactSource) {
		return [exactSource.sourceId];
	}

	return resolveDisjointAdditiveSources(completeDefinitions, targetMembers);
}

function ensureRequestedSourceDefinitions(
	definitions: SourceDefinition[],
	requestedSourceIds: string[]
): SourceDefinition[] {
	const definitionsBySource = new Map(
		definitions.map((definition) => [definition.sourceId, definition])
	);
	for (const sourceId of requestedSourceIds) {
		if (!definitionsBySource.has(sourceId)) {
			definitionsBySource.set(sourceId, { sourceId, members: [sourceId] });
		}
	}
	return [...definitionsBySource.values()].map((definition) => ({
		sourceId: definition.sourceId,
		members: uniqueSorted(definition.members)
	}));
}

function expandRequestedMembers(
	definitions: SourceDefinition[],
	requestedSourceIds: string[]
): string[] {
	const definitionsBySource = new Map(
		definitions.map((definition) => [definition.sourceId, definition])
	);
	const members = new Set<string>();
	for (const sourceId of requestedSourceIds) {
		const definition = definitionsBySource.get(sourceId);
		for (const memberId of definition?.members ?? [sourceId]) {
			members.add(memberId);
		}
	}
	return [...members].sort();
}

function findExactSourceForMembers(
	definitions: SourceDefinition[],
	targetMembers: string[],
	requestedSourceIds: string[]
): SourceDefinition | null {
	const matches = definitions.filter((definition) =>
		sameMembers(definition.members, targetMembers)
	);
	if (matches.length === 0) {
		return null;
	}

	const requested = new Set(requestedSourceIds);
	return [...matches].sort((left, right) => {
		const leftRequested = requested.has(left.sourceId) ? 0 : 1;
		const rightRequested = requested.has(right.sourceId) ? 0 : 1;
		return (
			leftRequested - rightRequested ||
			right.members.length - left.members.length ||
			left.sourceId.localeCompare(right.sourceId)
		);
	})[0];
}

function resolveDisjointAdditiveSources(
	definitions: SourceDefinition[],
	targetMembers: string[]
): string[] {
	const remaining = new Set(targetMembers);
	const selected: string[] = [];
	const candidates = [...definitions].sort(
		(left, right) =>
			right.members.length - left.members.length || left.sourceId.localeCompare(right.sourceId)
	);

	for (const candidate of candidates) {
		if (
			candidate.members.length === 0 ||
			!candidate.members.every((memberId) => remaining.has(memberId))
		) {
			continue;
		}
		selected.push(candidate.sourceId);
		for (const memberId of candidate.members) {
			remaining.delete(memberId);
		}
	}

	selected.push(...[...remaining].sort());
	return selected.sort();
}

function sameMembers(left: string[], right: string[]): boolean {
	const normalizedLeft = uniqueSorted(left);
	const normalizedRight = uniqueSorted(right);
	return (
		normalizedLeft.length === normalizedRight.length &&
		normalizedLeft.every((memberId, index) => memberId === normalizedRight[index])
	);
}

function uniqueSorted(values: string[]): string[] {
	return [...new Set(values)].sort();
}

export function groupByToGranularity(groupBy: string): IpGranularity {
	if (groupBy === 'date') return '1d';
	if (groupBy === 'hour') return '1h';
	if (groupBy === '30min') return '30m';
	return FIVE_MINUTE_GRANULARITY;
}

export function getBucketStartQuery(columnName: string, groupBy: string): string {
	const granularity = groupByToGranularity(groupBy);
	if (granularity === '5m') {
		return columnName;
	}

	const bucketSize = granularity === '30m' ? 1800 : granularity === '1h' ? 3600 : 86400;
	return `(CAST(strftime('%s', datetime(${columnName}, 'unixepoch', 'localtime', 'start of day', 'utc', printf('+%d seconds', ((CAST(strftime('%s', datetime(${columnName}, 'unixepoch', 'localtime')) AS integer) - CAST(strftime('%s', datetime(${columnName}, 'unixepoch', 'localtime', 'start of day')) AS integer)) / ${bucketSize}) * ${bucketSize}))) AS integer))`;
}

export function normalizeStructurePoints(
	points: RawStructureFunctionPoint[]
): StructureFunctionPoint[] {
	return points.map((point) => ({
		q: point.q,
		tau: point.tau ?? point.tauTilde ?? 0,
		sd: point.sd ?? point.s ?? 0
	}));
}
