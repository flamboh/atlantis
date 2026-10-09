<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import ChecklistPopover from '#lib/components/common/ChecklistPopover.svelte';
	import SegmentedToggle from '#lib/components/common/SegmentedToggle.svelte';
	import { sourceSeriesColor } from './chart-colors';
	import ChartCard from './ChartCard.svelte';
	import MetricLinePanel, { type MetricLineSeries } from './MetricLinePanel.svelte';
	import {
		getSourceLineDash,
		indexPortTimelines,
		type IndexedPortBucket
	} from './flow-characteristics';
	import type { GroupByOption } from '#lib/components/netflow/types.ts';
	import type {
		FlowCharacteristicsResponse,
		IpGranularity,
		NetflowIpFamily,
		PortRange,
		PortSide
	} from '#lib/types/types.ts';

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
		'10min': '10m',
		'5min': '5m'
	};
	const PORT_COLORS: Record<`${PortSide}-${PortRange}`, string> = {
		'source-low': 'var(--chart-series-1)',
		'source-high': 'var(--chart-series-3)',
		'destination-low': 'var(--chart-series-2)',
		'destination-high': 'var(--chart-series-4)'
	};
	const PORT_OPTIONS: Array<{ side: PortSide; range: PortRange; label: string }> = [
		{ side: 'source', range: 'low', label: 'Source ports 0-1023' },
		{ side: 'source', range: 'high', label: 'Source ports >1023' },
		{ side: 'destination', range: 'low', label: 'Destination ports 0-1023' },
		{ side: 'destination', range: 'high', label: 'Destination ports >1023' }
	];

	const PORT_FAMILY_OPTIONS = [
		{ value: 'ipv4', label: 'IPv4' },
		{ value: 'ipv6', label: 'IPv6' }
	] as const;
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
						color: sourceSeriesColor(PORT_COLORS[`${side}-${range}`], sourceIndex),
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
		<SegmentedToggle
			options={PORT_FAMILY_OPTIONS}
			value={portFamily}
			onValueChange={(family) => (portFamily = family)}
			ariaLabel="Port IP family"
		/>
		<ChecklistPopover
			label="Ranges"
			dialogLabel="Port ranges"
			items={PORT_OPTIONS.map((option) => ({
				key: `${option.side}-${option.range}` as const,
				label: option.label,
				checked: activePortSeries.has(`${option.side}-${option.range}`)
			}))}
			onToggle={(key) => {
				const option = PORT_OPTIONS.find(({ side, range }) => `${side}-${range}` === key);
				if (option) togglePortSeries(option.side, option.range);
			}}
			onSetAll={(checked) => {
				activePortSeries.clear();
				if (checked)
					for (const { side, range } of PORT_OPTIONS) activePortSeries.add(`${side}-${range}`);
			}}
		/>
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
