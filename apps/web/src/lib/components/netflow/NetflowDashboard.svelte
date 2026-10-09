<script lang="ts">
	import ChartLoading from '#lib/components/charts/ChartLoading.svelte';
	import ChartCardHeader from '#lib/components/charts/ChartCardHeader.svelte';
	import * as Card from '#lib/components/ui/card/index.ts';
	import { goto } from '$app/navigation';
	import ChartContainer from '#lib/components/charts/ChartContainer.svelte';
	import TrafficMetricControls from '#lib/components/filters/TrafficMetricControls.svelte';
	import SegmentedToggle from '#lib/components/common/SegmentedToggle.svelte';
	import { navigateToNetflowFile } from '#lib/utils/netflow-file-navigation.ts';
	import type { NetflowStatsData } from './netflow-stats-data.svelte.ts';
	import type { DataOption, GroupByOption, NetflowDataPoint, ChartTypeOption } from './types.ts';
	import type {
		FlowDirection,
		MaadIpVersion,
		MaadMeasure,
		NetflowIpFamily,
		NetflowMetricTotals,
		NetflowStatsResult
	} from '#lib/types/types.ts';

	type Props = {
		dataset: string;
		stats: NetflowStatsData;
		groupBy: GroupByOption;
		dataOptions: DataOption[];
		direction: FlowDirection;
		ipVersion?: MaadIpVersion;
		measure?: MaadMeasure;
		onDrillDown?: (payload: { groupBy: GroupByOption; startDate: string; endDate: string }) => void;
		onDataOptionsChange?: (payload: { options: DataOption[] }) => void;
	};
	const props: Props = $props();
	const IP_FAMILY_LABELS: Record<NetflowIpFamily, string> = {
		all: 'All',
		ipv4: 'IPv4',
		ipv6: 'IPv6'
	};
	const CHART_TYPE_OPTIONS = [
		{ value: 'stacked', label: 'Stacked' },
		{ value: 'line', label: 'Line' }
	] as const;

	let chartType = $state<ChartTypeOption>('stacked');
	let requestedIpFamily = $state<NetflowIpFamily>('all');
	const availableIpFamilies = $derived(props.stats.availableIpFamilies);
	const selectedIpFamily = $derived(
		availableIpFamilies.includes(requestedIpFamily) ? requestedIpFamily : 'all'
	);
	const loading = $derived(props.stats.loading);
	const error = $derived(props.stats.error);

	function getMetricsForFamily(
		row: NetflowStatsResult,
		suffix: 'Ipv4' | 'Ipv6' | null
	): NetflowMetricTotals {
		return {
			flows: suffix ? (row[`flows${suffix}`] ?? 0) : row.flows,
			flowsTcp: suffix ? (row[`flowsTcp${suffix}`] ?? 0) : row.flowsTcp,
			flowsUdp: suffix ? (row[`flowsUdp${suffix}`] ?? 0) : row.flowsUdp,
			flowsIcmp: suffix ? (row[`flowsIcmp${suffix}`] ?? 0) : row.flowsIcmp,
			flowsOther: suffix ? (row[`flowsOther${suffix}`] ?? 0) : row.flowsOther,
			packets: suffix ? (row[`packets${suffix}`] ?? 0) : row.packets,
			packetsTcp: suffix ? (row[`packetsTcp${suffix}`] ?? 0) : row.packetsTcp,
			packetsUdp: suffix ? (row[`packetsUdp${suffix}`] ?? 0) : row.packetsUdp,
			packetsIcmp: suffix ? (row[`packetsIcmp${suffix}`] ?? 0) : row.packetsIcmp,
			packetsOther: suffix ? (row[`packetsOther${suffix}`] ?? 0) : row.packetsOther,
			bytes: suffix ? (row[`bytes${suffix}`] ?? 0) : row.bytes,
			bytesTcp: suffix ? (row[`bytesTcp${suffix}`] ?? 0) : row.bytesTcp,
			bytesUdp: suffix ? (row[`bytesUdp${suffix}`] ?? 0) : row.bytesUdp,
			bytesIcmp: suffix ? (row[`bytesIcmp${suffix}`] ?? 0) : row.bytesIcmp,
			bytesOther: suffix ? (row[`bytesOther${suffix}`] ?? 0) : row.bytesOther
		};
	}

	const results = $derived.by<NetflowDataPoint[]>(() => {
		const suffix =
			selectedIpFamily === 'all' ? null : selectedIpFamily === 'ipv4' ? 'Ipv4' : 'Ipv6';
		return props.stats.results.map((bucket) => ({
			...bucket,
			data: bucket.data === null ? null : getMetricsForFamily(bucket.data, suffix)
		}));
	});

	const ipFamilyOptions = $derived(
		availableIpFamilies.length > 1
			? availableIpFamilies.map((family) => ({ value: family, label: IP_FAMILY_LABELS[family] }))
			: []
	);

	function handleDrillDown(newGroupBy: GroupByOption, newStartDate: string, newEndDate: string) {
		props.onDrillDown?.({ groupBy: newGroupBy, startDate: newStartDate, endDate: newEndDate });
	}

	function handleNavigateToFile(slug: string) {
		void navigateToNetflowFile(
			goto,
			slug,
			props.dataset,
			props.direction,
			props.ipVersion,
			props.measure
		);
	}
</script>

<Card.Root
	size="sm"
	class="gap-0 py-0"
	data-testid="chart-card-state"
	data-state={loading ? 'loading' : error ? 'error' : results.length === 0 ? 'empty' : 'ready'}
>
	<ChartCardHeader title="Traffic Overview">
		{#snippet controls()}
			<TrafficMetricControls
				dataOptions={props.dataOptions}
				onDataOptionsChange={(options) => props.onDataOptionsChange?.({ options })}
			/>
			{#if ipFamilyOptions.length > 0}
				<SegmentedToggle
					options={ipFamilyOptions}
					value={selectedIpFamily}
					onValueChange={(family) => (requestedIpFamily = family)}
					ariaLabel="Traffic IP family"
				/>
			{/if}
			<SegmentedToggle
				options={CHART_TYPE_OPTIONS}
				value={chartType}
				onValueChange={(next) => (chartType = next)}
				ariaLabel="Traffic chart type"
			/>
		{/snippet}
	</ChartCardHeader>

	<Card.Content class="p-0">
		<div
			class="chart-frame traffic-frame h-[380px] min-h-[280px] resize-none overflow-hidden p-3 [--chart-height:400px] sm:[--chart-height:520px] md:h-[320px] md:min-h-[240px] md:resize-y md:overflow-auto"
		>
			{#if loading}
				<ChartLoading label="Loading data..." />
			{:else if error}
				<div class="flex h-full items-center justify-center">
					<div class="text-destructive text-sm">{error}</div>
				</div>
			{:else if results.length === 0}
				<div class="flex h-full items-center justify-center">
					<div class="text-muted-foreground text-sm">
						No data available for the selected filters
					</div>
				</div>
			{:else}
				<ChartContainer
					{results}
					groupBy={props.groupBy}
					{chartType}
					dataOptions={props.dataOptions}
					onDrillDown={handleDrillDown}
					onNavigateToFile={handleNavigateToFile}
				/>
			{/if}
		</div>
	</Card.Content>
</Card.Root>
