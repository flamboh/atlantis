<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import ChartCard from './ChartCard.svelte';
	import MetricLinePanel, { type MetricLineSeries } from './MetricLinePanel.svelte';
	import {
		getSourceLineDash,
		indexPortTimelines,
		type IndexedPortBucket
	} from './flow-characteristics';
	import type { GroupByOption } from '$lib/components/netflow/types';
	import type {
		FlowCharacteristicsResponse,
		IpGranularity,
		NetflowIpFamily,
		PortRange,
		PortSide
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
	const PORT_COLORS: Record<`${PortSide}-${PortRange}`, string> = {
		'source-low': '#2563eb',
		'source-high': '#0891b2',
		'destination-low': '#d97706',
		'destination-high': '#dc2626'
	};
	const PORT_OPTIONS: Array<{ side: PortSide; range: PortRange; label: string }> = [
		{ side: 'source', range: 'low', label: 'Source ports 0-1023' },
		{ side: 'source', range: 'high', label: 'Source ports >1023' },
		{ side: 'destination', range: 'low', label: 'Destination ports 0-1023' },
		{ side: 'destination', range: 'high', label: 'Destination ports >1023' }
	];

	let portFamily = $state<Exclude<NetflowIpFamily, 'all'>>('ipv4');
	const activePortSeries = new SvelteSet(PORT_OPTIONS.map(({ side, range }) => `${side}-${range}`));
	const granularity = $derived(GROUP_BY_TO_GRANULARITY[props.groupBy]);
	const portIndex = $derived(indexPortTimelines(props.data?.portTimelines ?? []));
	const portStarts = $derived(portIndex.starts);
	const portSeries = $derived.by<MetricLineSeries[]>(() => {
		const multipleSources = (props.data?.resolvedSources.length ?? 0) > 1;
		return (props.data?.resolvedSources ?? []).flatMap((sourceId, sourceIndex) =>
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
</script>

<ChartCard
	title="Unique Ports"
	loading={props.loading}
	error={props.error}
	noMetrics={activePortSeries.size === 0}
	empty={portStarts.length === 0}
	loadingCopy="Loading port data..."
	noMetricsCopy="Select at least one port range"
	emptyCopy="No port data for the selected filters"
>
	{#snippet controls()}
		<div class="flex flex-wrap items-center gap-x-6 gap-y-3">
			<div class="flex flex-wrap items-center gap-4" role="group" aria-label="Port IP family">
				{#each ['ipv4', 'ipv6'] as const as family (family)}
					<label class="text-foreground flex cursor-pointer items-center gap-2 text-sm">
						<input
							type="radio"
							name="port-cardinality-ip-family"
							checked={portFamily === family}
							onchange={() => (portFamily = family)}
							class="border-input accent-primary focus-visible:ring-ring size-4 focus-visible:ring-2"
						/>
						<span>{family.toUpperCase()}</span>
					</label>
				{/each}
			</div>
			<div
				class="flex flex-wrap items-center gap-x-4 gap-y-2"
				role="group"
				aria-label="Port ranges"
			>
				{#each PORT_OPTIONS as option (`${option.side}-${option.range}`)}
					<label class="text-foreground flex cursor-pointer items-center gap-2 text-sm">
						<Checkbox
							checked={activePortSeries.has(`${option.side}-${option.range}`)}
							onCheckedChange={() => togglePortSeries(option.side, option.range)}
						/>
						<span>{option.label}</span>
					</label>
				{/each}
			</div>
		</div>
	{/snippet}

	<MetricLinePanel
		chartId="port-cardinality"
		title="Unique Ports"
		hideTitle
		yAxisTitle="Unique ports"
		bucketStarts={portStarts}
		{granularity}
		groupBy={props.groupBy}
		series={portSeries}
		valueFormat="integer"
		onDrillDown={props.onDrillDown}
		onNavigateToFile={props.onNavigateToFile}
	/>
</ChartCard>
