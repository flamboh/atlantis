import {
	DEFAULT_MAAD_IP_VERSION,
	DEFAULT_MAAD_MEASURE,
	FLOW_DIRECTIONS,
	flowDirectionLocalities,
	IP_GRANULARITIES,
	MAAD_IP_VERSIONS,
	MAAD_MEASURES,
	type FlowDirection,
	type FlowLocality,
	type FlowLocalityPair,
	type IpGranularity,
	type MaadIpVersion,
	type MaadMeasure
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
	direction: FlowDirection;
	srcLocality: FlowLocality;
	dstLocality: FlowLocality;
}

export interface MaadParams {
	ipVersion: MaadIpVersion;
	measure: MaadMeasure;
}

export type MaadStatsParams = AggregateStatsParams & MaadParams;

export interface RequestValidationError {
	error: string;
	status: 400;
}

export const FIVE_MINUTE_GRANULARITY: IpGranularity = '5m';
export const DEFAULT_IP_GRANULARITY: IpGranularity = '1h';
export type NetflowSchemaVersion = 'v3';

const VALID_IP_GRANULARITIES = new Set<string>(IP_GRANULARITIES);
const VALID_FLOW_DIRECTIONS = new Set<string>(FLOW_DIRECTIONS);
const VALID_MAAD_MEASURES = new Set<string>(MAAD_MEASURES);

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

export function parseFlowDirection(param: string | null): FlowDirection | null {
	if (!param) {
		return null;
	}

	return VALID_FLOW_DIRECTIONS.has(param) ? (param as FlowDirection) : null;
}

export function parseDirectionParam(param: string | null): FlowDirection | RequestValidationError {
	if (!param) {
		return 'all';
	}

	const direction = parseFlowDirection(param);
	if (direction) {
		return direction;
	}

	return {
		error: `Invalid direction. Expected one of: ${FLOW_DIRECTIONS.join(', ')}`,
		status: 400
	};
}

export function parseMaadIpVersion(url: URL): MaadIpVersion | RequestValidationError {
	const param = url.searchParams.get('ipVersion');
	if (param === null) {
		return DEFAULT_MAAD_IP_VERSION;
	}

	const version = MAAD_IP_VERSIONS.find((candidate) => String(candidate) === param);
	if (version === undefined) {
		return {
			error: `Invalid ipVersion. Expected one of: ${MAAD_IP_VERSIONS.join(', ')}`,
			status: 400
		};
	}

	return version;
}

export function parseMaadMeasure(url: URL): MaadMeasure | RequestValidationError {
	const param = url.searchParams.get('measure');
	if (param === null) {
		return DEFAULT_MAAD_MEASURE;
	}

	if (!VALID_MAAD_MEASURES.has(param)) {
		return {
			error: `Invalid measure. Expected one of: ${MAAD_MEASURES.join(', ')}`,
			status: 400
		};
	}

	return param as MaadMeasure;
}

export function parseMaadParams(url: URL): MaadParams | RequestValidationError {
	const ipVersion = parseMaadIpVersion(url);
	if (typeof ipVersion !== 'number') {
		return ipVersion;
	}

	const measure = parseMaadMeasure(url);
	if (typeof measure !== 'string') {
		return measure;
	}

	return { ipVersion, measure };
}

export function parseMaadStatsParams(url: URL): MaadStatsParams | RequestValidationError {
	const aggregate = parseAggregateStatsParams(url);
	if ('error' in aggregate) {
		return aggregate;
	}

	const maad = parseMaadParams(url);
	if ('error' in maad) {
		return maad;
	}

	return { ...aggregate, ...maad };
}

export function spectrumMeasureError(measure: MaadMeasure): RequestValidationError | null {
	return measure === 'addresses'
		? null
		: { error: `Spectrum is only computed for the addresses measure, not ${measure}`, status: 400 };
}

export function parseFlowDirectionParams(
	url: URL
): ({ direction: FlowDirection } & FlowLocalityPair) | RequestValidationError {
	const direction = parseDirectionParam(url.searchParams.get('direction'));

	if (typeof direction !== 'string') {
		return direction;
	}

	return { direction, ...flowDirectionLocalities(direction) };
}

export function parseAggregateStatsParams(url: URL): AggregateStatsParams | RequestValidationError {
	const routers = parseSourceIds(url.searchParams.get('routers'));
	const granularityParam = url.searchParams.get('granularity');
	const parsedGranularity = parseIpGranularity(granularityParam);
	const granularity = parsedGranularity ?? DEFAULT_IP_GRANULARITY;
	const start = parseTimestamp(url.searchParams.get('startDate'));
	const end = parseTimestamp(url.searchParams.get('endDate'));
	const flowDirection = parseFlowDirectionParams(url);

	if (routers.length === 0) {
		return { error: 'No routers selected', status: 400 };
	}

	if (granularityParam !== null && parsedGranularity === null) {
		return {
			error: `Invalid granularity. Expected one of: ${IP_GRANULARITIES.join(', ')}`,
			status: 400
		};
	}

	if ('error' in flowDirection) {
		return flowDirection;
	}

	if (start === null || end === null) {
		return { error: 'Invalid start or end time', status: 400 };
	}

	if (start >= end) {
		return { error: 'Start time must be before end time', status: 400 };
	}

	return {
		routers,
		granularity,
		start,
		end,
		direction: flowDirection.direction,
		srcLocality: flowDirection.srcLocality,
		dstLocality: flowDirection.dstLocality
	};
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
	if (groupBy === '10min') return '10m';
	return FIVE_MINUTE_GRANULARITY;
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
