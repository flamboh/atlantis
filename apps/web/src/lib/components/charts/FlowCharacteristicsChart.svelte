<script module lang="ts">
	import { SvelteMap, SvelteSet } from 'svelte/reactivity';
	import type {
		BucketCoverage,
		NetflowIpFamily,
		ObservationStats,
		PortCardinalityCounts,
		PortCardinalityTimeline,
		TimeBucket
	} from '$lib/types/types';

	export type IndexedObservationBucket = {
		coverage: BucketCoverage;
		byFamily: Map<NetflowIpFamily, ObservationStats>;
	};

	export type IndexedPortBucket = {
		coverage: BucketCoverage;
		values: PortCardinalityCounts | null;
	};

	export function indexObservationBuckets(buckets: TimeBucket<ObservationStats[]>[]) {
		const byStart = new SvelteMap<number, IndexedObservationBucket>();
		for (const bucket of buckets) {
			byStart.set(bucket.bucketStart, {
				coverage: bucket.coverage,
				byFamily: new SvelteMap((bucket.data ?? []).map((row) => [row.ipFamily, row]))
			});
		}
		return { starts: [...byStart.keys()].sort((left, right) => left - right), byStart };
	}

	export function indexPortTimelines(timelines: PortCardinalityTimeline[]) {
		const starts = new SvelteSet<number>();
		const bySource = new SvelteMap<string, Map<number, IndexedPortBucket>>();
		for (const timeline of timelines) {
			const bucketsByStart = bySource.get(timeline.sourceId) ?? new SvelteMap();
			bySource.set(timeline.sourceId, bucketsByStart);
			for (const bucket of timeline.buckets) {
				starts.add(bucket.bucketStart);
				bucketsByStart.set(bucket.bucketStart, {
					coverage: bucket.coverage,
					values: bucket.data
				});
			}
		}
		return { starts: [...starts].sort((left, right) => left - right), bySource };
	}
</script>

<script lang="ts">
	import { onDestroy } from 'svelte';
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import MetricLinePanel, { type MetricLineSeries } from './MetricLinePanel.svelte';
	import { dateStringToEpochPST } from '$lib/utils/timezone';
	import {
		ensureCachedWindow,
		getMissingWindowRanges,
		readCachedWindow,
		type TimeRange
	} from '$lib/utils/window-cache';
	import { watch } from 'runed';
	import { createRequestGate, getSourceLineDash } from './flow-characteristics';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types';
	import type {
		FlowCharacteristicsResponse,
		FlowVisibility,
		IpGranularity,
		PortRange,
		PortSide
	} from '$lib/types/types';

	const props = $props<{
		dataset: string;
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		routersLoaded: boolean;
		srcVisibility: FlowVisibility;
		dstVisibility: FlowVisibility;
	}>();

	const GROUP_BY_TO_GRANULARITY: Record<GroupByOption, IpGranularity> = {
		date: '1d',
		hour: '1h',
		'30min': '30m',
		'5min': '5m'
	};
	const PORT_COLORS: Record<`${PortSide}-${PortRange}`, string> = {
		'source-low': '#2563eb',
		'source-high': '#0891b2',
		'destination-low': '#d97706',
		'destination-high': '#dc2626'
	};
	const PORT_OPTIONS: Array<{ side: PortSide; range: PortRange; label: string }> = [
		{ side: 'source', range: 'low', label: 'Source · 0–1023' },
		{ side: 'source', range: 'high', label: 'Source · >1023' },
		{ side: 'destination', range: 'low', label: 'Destination · 0–1023' },
		{ side: 'destination', range: 'high', label: 'Destination · >1023' }
	];

	let observationFamily = $state<NetflowIpFamily>('all');
	let portFamily = $state<Exclude<NetflowIpFamily, 'all'>>('ipv4');
	const activePortSeries = new SvelteSet(PORT_OPTIONS.map(({ side, range }) => `${side}-${range}`));
	let data = $state.raw<FlowCharacteristicsResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	const requestGate = createRequestGate();
	let requestController: AbortController | null = null;

	type CachedCharacteristicsRecord =
		| { kind: 'source'; sourceId: string; sourceIndex: number }
		| { kind: 'observation'; bucket: TimeBucket<ObservationStats[]> }
		| {
				kind: 'port';
				sourceId: string;
				bucket: TimeBucket<PortCardinalityCounts>;
		  };

	const granularity: IpGranularity = $derived(
		GROUP_BY_TO_GRANULARITY[props.groupBy as GroupByOption]
	);
	const observationIndex = $derived(indexObservationBuckets(data?.observationBuckets ?? []));
	const observationStarts = $derived(observationIndex.starts);
	const observationCoverage = $derived<BucketCoverage[]>(
		observationStarts.map(
			(start) =>
				observationIndex.byStart.get(start)?.coverage ?? {
					state: 'unknown',
					observedUnits: 0,
					expectedUnits: 0
				}
		)
	);
	const durationSeries = $derived<MetricLineSeries[]>([
		{
			label: 'Average duration',
			values: observationValuesByStart(
				observationIndex.byStart,
				observationStarts,
				observationFamily,
				'averageDurationMs'
			),
			color: '#2563eb',
			coverage: observationCoverage
		}
	]);
	const ttlSeries = $derived<MetricLineSeries[]>([
		{
			label: 'Average minimum TTL',
			values: observationValuesByStart(
				observationIndex.byStart,
				observationStarts,
				observationFamily,
				'averageMinTtl'
			),
			color: '#7c3aed',
			coverage: observationCoverage
		},
		{
			label: 'Average maximum TTL',
			values: observationValuesByStart(
				observationIndex.byStart,
				observationStarts,
				observationFamily,
				'averageMaxTtl'
			),
			color: '#db2777',
			coverage: observationCoverage
		}
	]);
	const portIndex = $derived(indexPortTimelines(data?.portTimelines ?? []));
	const portStarts = $derived(portIndex.starts);
	const portSeries = $derived.by<MetricLineSeries[]>(() => {
		const multipleSources = (data?.resolvedSources.length ?? 0) > 1;
		return (data?.resolvedSources ?? []).flatMap((sourceId, sourceIndex) =>
			PORT_OPTIONS.filter(({ side, range }) => activePortSeries.has(`${side}-${range}`)).map(
				({ side, range, label }) => {
					const timeline = portIndex.bySource.get(sourceId);
					return {
						label: multipleSources ? `${sourceId} · ${label}` : label,
						values: portValuesByStart(timeline, portStarts, portFamily, side, range),
						color: PORT_COLORS[`${side}-${range}`],
						dash: getSourceLineDash(sourceIndex, multipleSources),
						coverage: portStarts.map(
							(start) =>
								timeline?.get(start)?.coverage ?? {
									state: 'unknown',
									observedUnits: 0,
									expectedUnits: 0
								}
						)
					};
				}
			)
		);
	});

	function selectedRouters(): string[] {
		return Object.entries(props.routers)
			.filter(([, enabled]) => enabled)
			.map(([sourceId]) => sourceId.trim())
			.filter(Boolean)
			.sort();
	}

	function observationValuesByStart(
		bucketsByStart: Map<number, IndexedObservationBucket>,
		starts: number[],
		family: NetflowIpFamily,
		key: 'averageDurationMs' | 'averageMinTtl' | 'averageMaxTtl'
	): Array<number | null> {
		return starts.map((start) => bucketsByStart.get(start)?.byFamily.get(family)?.[key] ?? null);
	}

	function portValuesByStart(
		bucketsByStart: Map<number, IndexedPortBucket> | undefined,
		starts: number[],
		family: Exclude<NetflowIpFamily, 'all'>,
		side: PortSide,
		range: PortRange
	): Array<number | null> {
		return starts.map((start) => {
			const bucket = bucketsByStart?.get(start);
			if (!bucket?.values) return null;
			return bucket.values[family][side][range];
		});
	}

	function togglePortSeries(side: PortSide, range: PortRange) {
		const key = `${side}-${range}`;
		if (activePortSeries.has(key)) {
			activePortSeries.delete(key);
		} else {
			activePortSeries.add(key);
		}
	}

	function getCacheKey(routers: string[]): string {
		return JSON.stringify({
			chart: 'flow-characteristics',
			dataset: props.dataset,
			granularity,
			routers,
			srcVisibility: props.srcVisibility,
			dstVisibility: props.dstVisibility
		});
	}

	function recordStart(record: CachedCharacteristicsRecord): number {
		return record.kind === 'source' ? Number.NEGATIVE_INFINITY : record.bucket.bucketStart;
	}

	function readCachedData(
		cacheKey: string,
		requestedRange: TimeRange
	): FlowCharacteristicsResponse {
		const records = readCachedWindow<CachedCharacteristicsRecord>(
			cacheKey,
			requestedRange,
			(record, range) =>
				record.kind === 'source' ||
				(record.bucket.bucketStart >= range.start && record.bucket.bucketStart < range.end)
		);
		const sources = records
			.filter(
				(record): record is Extract<CachedCharacteristicsRecord, { kind: 'source' }> =>
					record.kind === 'source'
			)
			.sort(
				(left, right) =>
					left.sourceIndex - right.sourceIndex || left.sourceId.localeCompare(right.sourceId)
			);
		const portBuckets = new SvelteMap<string, TimeBucket<PortCardinalityCounts>[]>();
		for (const source of sources) {
			portBuckets.set(source.sourceId, []);
		}
		for (const record of records) {
			if (record.kind !== 'port') continue;
			const buckets = portBuckets.get(record.sourceId) ?? [];
			buckets.push(record.bucket);
			portBuckets.set(record.sourceId, buckets);
		}

		return {
			observationBuckets: records
				.filter(
					(record): record is Extract<CachedCharacteristicsRecord, { kind: 'observation' }> =>
						record.kind === 'observation'
				)
				.map((record) => record.bucket)
				.sort((left, right) => left.bucketStart - right.bucketStart),
			portTimelines: [...portBuckets].map(([sourceId, buckets]) => ({
				sourceId,
				buckets: buckets.sort((left, right) => left.bucketStart - right.bucketStart)
			})),
			resolvedSources: sources.map((source) => source.sourceId)
		};
	}

	async function loadData() {
		const token = requestGate.begin();
		requestController?.abort();
		requestController = null;
		if (!props.routersLoaded) {
			loading = true;
			return;
		}
		const routers = selectedRouters();
		if (routers.length === 0) {
			data = null;
			error = 'Select at least one source to view flow characteristics';
			loading = false;
			return;
		}
		const requestedRange = {
			start: dateStringToEpochPST(props.startDate),
			end: dateStringToEpochPST(props.endDate, true)
		};
		const cacheKey = getCacheKey(routers);
		loading = getMissingWindowRanges(cacheKey, requestedRange).length > 0;
		error = null;
		const baseParams = new URLSearchParams({
			dataset: props.dataset,
			routers: routers.join(','),
			granularity,
			srcVisibility: props.srcVisibility,
			dstVisibility: props.dstVisibility
		});
		const controller = new AbortController();
		requestController = controller;
		try {
			await ensureCachedWindow<CachedCharacteristicsRecord>({
				key: cacheKey,
				requestedRange,
				signal: controller.signal,
				fetchRange: async (range, signal) => {
					const response = await fetch(
						`/api/netflow/characteristics?${new URLSearchParams({
							...Object.fromEntries(baseParams.entries()),
							startDate: range.start.toString(),
							endDate: range.end.toString()
						}).toString()}`,
						{ signal }
					);
					if (!response.ok) throw new Error((await response.text()) || 'Request failed');
					const next = (await response.json()) as FlowCharacteristicsResponse;
					return [
						...next.resolvedSources.map(
							(sourceId, sourceIndex): CachedCharacteristicsRecord => ({
								kind: 'source',
								sourceId,
								sourceIndex
							})
						),
						...next.observationBuckets.map(
							(bucket): CachedCharacteristicsRecord => ({ kind: 'observation', bucket })
						),
						...next.portTimelines.flatMap((timeline) =>
							timeline.buckets.map(
								(bucket): CachedCharacteristicsRecord => ({
									kind: 'port',
									sourceId: timeline.sourceId,
									bucket
								})
							)
						)
					];
				},
				getRecordKey: (record) => {
					if (record.kind === 'source') return `source:${record.sourceId}`;
					if (record.kind === 'observation') return `observation:${record.bucket.bucketStart}`;
					return `port:${record.sourceId}:${record.bucket.bucketStart}`;
				},
				compareRecords: (left, right) =>
					recordStart(left) - recordStart(right) ||
					(left.kind === 'port' ? left.sourceId : '').localeCompare(
						right.kind === 'port' ? right.sourceId : ''
					)
			});
			if (requestGate.isCurrent(token)) data = readCachedData(cacheKey, requestedRange);
		} catch (reason) {
			if (requestGate.isCurrent(token)) {
				if (reason instanceof DOMException && reason.name === 'AbortError') return;
				data = null;
				error = reason instanceof Error ? reason.message : 'Failed to load flow characteristics';
			}
		} finally {
			if (requestGate.isCurrent(token)) {
				loading = false;
				if (requestController === controller) requestController = null;
			}
		}
	}

	watch(
		() =>
			JSON.stringify({
				dataset: props.dataset,
				startDate: props.startDate,
				endDate: props.endDate,
				groupBy: props.groupBy,
				routers: props.routers,
				routersLoaded: props.routersLoaded,
				srcVisibility: props.srcVisibility,
				dstVisibility: props.dstVisibility
			}),
		() => void loadData()
	);

	onDestroy(() => {
		requestGate.begin();
		requestController?.abort();
	});
</script>

<div class="border-border bg-card text-card-foreground rounded-lg border shadow-sm">
	<div
		class="border-border relative cursor-grab border-b p-4 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<h3 class="text-foreground text-lg font-semibold">Flow Characteristics</h3>
		<p class="text-muted-foreground mt-1 text-sm">
			Weighted flow observations and exact unique port counts
		</p>
		<DragGrip />
	</div>

	<div class="space-y-5 p-4">
		{#if loading}
			<div class="text-muted-foreground flex min-h-72 items-center justify-center">
				Loading flow characteristics…
			</div>
		{:else if error}
			<div class="text-destructive flex min-h-72 items-center justify-center">
				{error}
			</div>
		{:else}
			<div class="flex flex-wrap items-center justify-between gap-3">
				<h4 class="text-foreground text-sm font-semibold">Observations</h4>
				<div
					class="border-border bg-muted flex rounded-md border p-1"
					role="group"
					aria-label="Observation IP family"
				>
					{#each ['all', 'ipv4', 'ipv6'] as const as family (family)}
						<button
							type="button"
							class={`min-h-8 rounded px-3 text-xs font-medium ${observationFamily === family ? 'bg-primary text-primary-foreground' : 'text-muted-foreground'}`}
							aria-pressed={observationFamily === family}
							onclick={() => (observationFamily = family)}
						>
							{family === 'all' ? 'All' : family.toUpperCase()}
						</button>
					{/each}
				</div>
			</div>

			<div class="grid gap-5 xl:grid-cols-2">
				<MetricLinePanel
					title="Average Flow Duration"
					yAxisTitle="Duration"
					bucketStarts={observationStarts}
					{granularity}
					series={durationSeries}
					valueFormat="duration"
				/>
				<MetricLinePanel
					title="Average TTL"
					yAxisTitle="TTL (hops)"
					bucketStarts={observationStarts}
					{granularity}
					series={ttlSeries}
					valueFormat="decimal"
				/>
			</div>

			<div class="border-border border-t pt-5">
				<div class="mb-4 flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
					<div>
						<h4 class="text-foreground text-sm font-semibold">Unique Ports</h4>
						<p class="text-muted-foreground text-xs">
							Cardinality is resolved from an exact logical source; separate sources are never
							added.
						</p>
					</div>
					<div
						class="border-border bg-muted flex rounded-md border p-1"
						role="group"
						aria-label="Port IP family"
					>
						{#each ['ipv4', 'ipv6'] as const as family (family)}
							<button
								type="button"
								class={`min-h-8 rounded px-3 text-xs font-medium ${portFamily === family ? 'bg-primary text-primary-foreground' : 'text-muted-foreground'}`}
								aria-pressed={portFamily === family}
								onclick={() => (portFamily = family)}>{family.toUpperCase()}</button
							>
						{/each}
					</div>
				</div>
				<div
					class="mb-4 grid gap-2 sm:grid-cols-2 xl:grid-cols-4"
					role="group"
					aria-label="Port cardinality series"
				>
					{#each PORT_OPTIONS as option (`${option.side}-${option.range}`)}
						<label
							class="border-border flex min-h-10 cursor-pointer items-center gap-2 rounded-md border px-3 text-sm"
						>
							<Checkbox
								checked={activePortSeries.has(`${option.side}-${option.range}`)}
								onCheckedChange={() => togglePortSeries(option.side, option.range)}
							/>
							<span class="text-foreground">{option.label}</span>
						</label>
					{/each}
				</div>
				<MetricLinePanel
					title="Port Cardinality"
					yAxisTitle="Unique ports"
					bucketStarts={portStarts}
					{granularity}
					series={portSeries}
					valueFormat="integer"
				/>
			</div>
		{/if}
	</div>
</div>
