<script lang="ts">
	import ChartCard from './ChartCard.svelte';
	import MetricLinePanel, { type MetricLineSeries } from './MetricLinePanel.svelte';
	import { indexObservationBuckets, type IndexedObservationBucket } from './flow-characteristics';
	import type { GroupByOption } from '$lib/components/netflow/types';
	import type {
		BucketCoverage,
		FlowCharacteristicsResponse,
		IpGranularity,
		NetflowIpFamily
	} from '$lib/types/types';

	type Props = {
		data: FlowCharacteristicsResponse | null;
		loading: boolean;
		error: string | null;
		groupBy: GroupByOption;
		onDrillDown?: (groupBy: GroupByOption, startDate: string, endDate: string) => void;
		onNavigateToFile?: (slug: string) => void;
	};

	const props: Props = $props();

	const GROUP_BY_TO_GRANULARITY: Record<GroupByOption, IpGranularity> = {
		date: '1d',
		hour: '1h',
		'30min': '30m',
		'5min': '5m'
	};
	const IP_FAMILY_OPTIONS: Array<{ value: NetflowIpFamily; label: string }> = [
		{ value: 'all', label: 'All' },
		{ value: 'ipv4', label: 'IPv4' },
		{ value: 'ipv6', label: 'IPv6' }
	];

	let observationFamily = $state<NetflowIpFamily>('all');
	const granularity = $derived(GROUP_BY_TO_GRANULARITY[props.groupBy]);
	const observationIndex = $derived(indexObservationBuckets(props.data?.observationBuckets ?? []));
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

	function observationValuesByStart(
		bucketsByStart: Map<number, IndexedObservationBucket>,
		starts: number[],
		family: NetflowIpFamily,
		key: 'averageDurationMs' | 'averageMinTtl' | 'averageMaxTtl'
	): Array<number | null> {
		return starts.map((start) => bucketsByStart.get(start)?.byFamily.get(family)?.[key] ?? null);
	}
</script>

<ChartCard
	title="Flow Characteristics"
	size="split"
	loading={props.loading}
	error={props.error}
	noMetrics={false}
	empty={observationStarts.length === 0}
	loadingCopy="Loading flow characteristics..."
	noMetricsCopy=""
	emptyCopy="No flow characteristics for the selected filters"
>
	{#snippet controls()}
		<div class="flex flex-wrap items-center gap-4" role="group" aria-label="IP family">
			{#each IP_FAMILY_OPTIONS as option (option.value)}
				<label class="text-foreground flex cursor-pointer items-center gap-2 text-sm">
					<input
						type="radio"
						name="flow-characteristics-ip-family"
						checked={observationFamily === option.value}
						onchange={() => (observationFamily = option.value)}
						class="border-input accent-primary focus-visible:ring-ring size-4 focus-visible:ring-2"
					/>
					<span>{option.label}</span>
				</label>
			{/each}
		</div>
	{/snippet}

	<div class="grid h-full gap-4 xl:grid-cols-2">
		<MetricLinePanel
			chartId="flow-duration"
			title="Average Flow Duration"
			yAxisTitle="Duration"
			bucketStarts={observationStarts}
			{granularity}
			groupBy={props.groupBy}
			series={durationSeries}
			valueFormat="duration"
			onDrillDown={props.onDrillDown}
			onNavigateToFile={props.onNavigateToFile}
		/>
		<MetricLinePanel
			chartId="flow-ttl"
			title="Average TTL"
			yAxisTitle="TTL (hops)"
			bucketStarts={observationStarts}
			{granularity}
			groupBy={props.groupBy}
			series={ttlSeries}
			valueFormat="decimal"
			onDrillDown={props.onDrillDown}
			onNavigateToFile={props.onNavigateToFile}
		/>
	</div>
</ChartCard>
