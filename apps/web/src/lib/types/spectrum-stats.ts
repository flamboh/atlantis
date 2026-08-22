import type { SpectrumPoint, TimeBucket } from './types';

export interface SpectrumStatsPayload {
	spectrumSa: SpectrumPoint[];
	spectrumDa: SpectrumPoint[];
}

export interface SpectrumStatsTimeline {
	router: string;
	buckets: TimeBucket<SpectrumStatsPayload>[];
}

export interface SpectrumStatsResponse {
	timelines: SpectrumStatsTimeline[];
	requestedRouters: string[];
}
