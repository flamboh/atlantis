import type { StructureFunctionPoint, TimeBucket } from './types';

export interface StructureStatsPayload {
	structureSa: StructureFunctionPoint[];
	structureDa: StructureFunctionPoint[];
}

export interface StructureStatsTimeline {
	router: string;
	buckets: TimeBucket<StructureStatsPayload>[];
}

export interface StructureStatsResponse {
	timelines: StructureStatsTimeline[];
	requestedRouters: string[];
}
