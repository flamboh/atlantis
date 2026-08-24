import { describe, expect, it } from 'vitest';
import {
	createRequestGate,
	getSourceLineDash
} from '../../src/lib/components/charts/flow-characteristics';
import {
	indexObservationBuckets,
	indexPortTimelines
} from '../../src/lib/components/charts/FlowCharacteristicsChart.svelte';
import type {
	ObservationStats,
	PortCardinalityCounts,
	PortCardinalityTimeline,
	TimeBucket
} from '../../src/lib/types/types';

describe('flow characteristics request coordination', () => {
	it('rejects an in-flight response after all sources are deselected', () => {
		const gate = createRequestGate();
		const inFlightRequest = gate.begin();

		gate.begin(); // loadData's empty-source early return

		expect(gate.isCurrent(inFlightRequest)).toBe(false);
	});

	it('rejects an in-flight response while a new dataset has not loaded its sources', () => {
		const gate = createRequestGate();
		const previousDatasetRequest = gate.begin();

		gate.begin(); // loadData's routersLoaded=false early return

		expect(gate.isCurrent(previousDatasetRequest)).toBe(false);
	});

	it('uses a distinct line pattern for each fallback source', () => {
		expect(getSourceLineDash(0, true)).not.toEqual(getSourceLineDash(1, true));
		expect(getSourceLineDash(0, false)).toEqual([]);
	});
});

describe('flow characteristics bucket indexes', () => {
	it('indexes observation dimensions once by bucket and family', () => {
		const buckets: TimeBucket<ObservationStats[]>[] = [
			{
				bucketStart: 100,
				bucketEnd: 200,
				coverage: { state: 'complete', observedUnits: 1, expectedUnits: 1 },
				data: [
					{
						ipFamily: 'ipv4',
						averageDurationMs: 12,
						averageMinTtl: 31,
						averageMaxTtl: 63
					}
				]
			}
		];

		const indexed = indexObservationBuckets(buckets);

		expect(indexed.starts).toEqual([100]);
		expect(indexed.byStart.get(100)?.byFamily.get('ipv4')?.averageDurationMs).toBe(12);
		expect(indexed.byStart.get(100)?.coverage.state).toBe('complete');
	});

	it('indexes port dimensions across sources without repeated bucket scans', () => {
		const bucket: TimeBucket<PortCardinalityCounts> = {
			bucketStart: 100,
			bucketEnd: 200,
			coverage: { state: 'partial', observedUnits: 1, expectedUnits: 2 },
			data: {
				ipv4: {
					source: { low: 17, high: 0 },
					destination: { low: 0, high: 0 }
				},
				ipv6: {
					source: { low: 0, high: 0 },
					destination: { low: 0, high: 0 }
				}
			}
		};
		const timelines: PortCardinalityTimeline[] = [
			{ sourceId: 'router-b', buckets: [bucket] },
			{ sourceId: 'router-a', buckets: [{ ...bucket, bucketStart: 50, bucketEnd: 100 }] }
		];

		const indexed = indexPortTimelines(timelines);

		expect(indexed.starts).toEqual([50, 100]);
		expect(indexed.bySource.get('router-b')?.get(100)?.values?.ipv4.source.low).toBe(17);
		expect(indexed.bySource.get('router-b')?.get(100)?.coverage.state).toBe('partial');
	});
});
