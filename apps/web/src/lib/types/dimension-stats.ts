import type { TimeBucket } from './types';

export const DIMENSION_METRIC_KEYS = ['saD0', 'saD1', 'saD2', 'daD0', 'daD1', 'daD2'] as const;

export type DimensionMetricKey = (typeof DIMENSION_METRIC_KEYS)[number];

export type DimensionStatsPayload = Record<DimensionMetricKey, number | null>;

export interface DimensionStatsTimeline {
	router: string;
	buckets: TimeBucket<DimensionStatsPayload>[];
}

export interface DimensionStatsResponse {
	timelines: DimensionStatsTimeline[];
	requestedRouters: string[];
}

export interface MaadStatusResponse {
	computed: boolean;
}
