<script lang="ts">
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import MetricLinePanel, { type MetricLineSeries } from './MetricLinePanel.svelte';
	import { dateStringToEpochPST } from '$lib/utils/timezone';
	import { watch } from 'runed';
	import { createRequestGate, getSourceLineDash } from './flow-characteristics';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types';
	import type {
		FlowCharacteristicsResponse,
		FlowVisibility,
		IpGranularity,
		NetflowIpFamily,
		ObservationStatsBucket,
		PortCardinalityBucket,
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
	let activePortSeries = $state(new Set(PORT_OPTIONS.map(({ side, range }) => `${side}-${range}`)));
	let data = $state<FlowCharacteristicsResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	const requestGate = createRequestGate();

	const granularity: IpGranularity = $derived(
		GROUP_BY_TO_GRANULARITY[props.groupBy as GroupByOption]
	);
	const observationRows = $derived(
		(data?.observationBuckets ?? []).filter((row) => row.ipFamily === observationFamily)
	);
	const observationStarts = $derived(uniqueStarts(observationRows));
	const durationSeries = $derived<MetricLineSeries[]>([
		{
			label: 'Average duration',
			values: valuesByStart(observationRows, observationStarts, 'averageDurationMs'),
			color: '#2563eb'
		}
	]);
	const ttlSeries = $derived<MetricLineSeries[]>([
		{
			label: 'Average minimum TTL',
			values: valuesByStart(observationRows, observationStarts, 'averageMinTtl'),
			color: '#7c3aed'
		},
		{
			label: 'Average maximum TTL',
			values: valuesByStart(observationRows, observationStarts, 'averageMaxTtl'),
			color: '#db2777'
		}
	]);
	const selectedPortRows = $derived(
		(data?.portBuckets ?? []).filter(
			(row) =>
				row.ipFamily === portFamily && activePortSeries.has(`${row.portSide}-${row.portRange}`)
		)
	);
	const portStarts = $derived(uniqueStarts(selectedPortRows));
	const portSeries = $derived.by<MetricLineSeries[]>(() => {
		const multipleSources = (data?.resolvedSources.length ?? 0) > 1;
		return (data?.resolvedSources ?? []).flatMap((sourceId, sourceIndex) =>
			PORT_OPTIONS.filter(({ side, range }) => activePortSeries.has(`${side}-${range}`)).map(
				({ side, range, label }) => ({
					label: multipleSources ? `${sourceId} · ${label}` : label,
					values: portValuesByStart(selectedPortRows, portStarts, sourceId, side, range),
					color: PORT_COLORS[`${side}-${range}`],
					dash: getSourceLineDash(sourceIndex, multipleSources)
				})
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

	function uniqueStarts(rows: Array<{ bucketStart: number }>): number[] {
		return [...new Set(rows.map((row) => row.bucketStart))].sort((left, right) => left - right);
	}

	function valuesByStart(
		rows: ObservationStatsBucket[],
		starts: number[],
		key: 'averageDurationMs' | 'averageMinTtl' | 'averageMaxTtl'
	): Array<number | null> {
		const values = new Map(rows.map((row) => [row.bucketStart, row[key]]));
		return starts.map((start) => values.get(start) ?? null);
	}

	function portValuesByStart(
		rows: PortCardinalityBucket[],
		starts: number[],
		sourceId: string,
		side: PortSide,
		range: PortRange
	): Array<number | null> {
		const values = new Map(
			rows
				.filter(
					(row) => row.sourceId === sourceId && row.portSide === side && row.portRange === range
				)
				.map((row) => [row.bucketStart, row.uniquePortCount])
		);
		return starts.map((start) => values.get(start) ?? null);
	}

	function togglePortSeries(side: PortSide, range: PortRange) {
		const key = `${side}-${range}`;
		const next = new Set(activePortSeries);
		if (next.has(key)) {
			next.delete(key);
		} else {
			next.add(key);
		}
		activePortSeries = next;
	}

	async function loadData() {
		const token = requestGate.begin();
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
		loading = true;
		error = null;
		const params = new URLSearchParams({
			dataset: props.dataset,
			routers: routers.join(','),
			granularity,
			startDate: dateStringToEpochPST(props.startDate).toString(),
			endDate: dateStringToEpochPST(props.endDate, true).toString(),
			srcVisibility: props.srcVisibility,
			dstVisibility: props.dstVisibility
		});
		try {
			const response = await fetch(`/api/netflow/characteristics?${params}`);
			if (!response.ok) throw new Error((await response.text()) || 'Request failed');
			const next = (await response.json()) as FlowCharacteristicsResponse;
			if (requestGate.isCurrent(token)) data = next;
		} catch (reason) {
			if (requestGate.isCurrent(token)) {
				data = null;
				error = reason instanceof Error ? reason.message : 'Failed to load flow characteristics';
			}
		} finally {
			if (requestGate.isCurrent(token)) loading = false;
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
</script>

<div class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white shadow-sm">
	<div
		class="dark:border-dark-border relative cursor-grab border-b p-4 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<h3 class="text-lg font-semibold text-gray-900 dark:text-gray-100">Flow Characteristics</h3>
		<p class="mt-1 text-sm text-gray-500 dark:text-gray-400">
			Weighted flow observations and exact unique port counts
		</p>
		<DragGrip />
	</div>

	<div class="space-y-5 p-4">
		{#if loading}
			<div class="flex min-h-72 items-center justify-center text-gray-500 dark:text-gray-400">
				Loading flow characteristics…
			</div>
		{:else if error}
			<div class="flex min-h-72 items-center justify-center text-red-600 dark:text-red-400">
				{error}
			</div>
		{:else}
			<div class="flex flex-wrap items-center justify-between gap-3">
				<h4 class="text-sm font-semibold text-gray-800 dark:text-gray-200">Observations</h4>
				<div
					class="dark:border-dark-border dark:bg-dark-subtle flex rounded-md border bg-gray-50 p-1"
					role="group"
					aria-label="Observation IP family"
				>
					{#each ['all', 'ipv4', 'ipv6'] as const as family (family)}
						<button
							type="button"
							class={`min-h-8 rounded px-3 text-xs font-medium ${observationFamily === family ? 'bg-blue-600 text-white' : 'text-gray-700 dark:text-gray-300'}`}
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

			<div class="dark:border-dark-border border-t pt-5">
				<div class="mb-4 flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
					<div>
						<h4 class="text-sm font-semibold text-gray-800 dark:text-gray-200">Unique Ports</h4>
						<p class="text-xs text-gray-500 dark:text-gray-400">
							Cardinality is resolved from an exact logical source; separate sources are never
							added.
						</p>
					</div>
					<div
						class="dark:border-dark-border dark:bg-dark-subtle flex rounded-md border bg-gray-50 p-1"
						role="group"
						aria-label="Port IP family"
					>
						{#each ['ipv4', 'ipv6'] as const as family (family)}
							<button
								type="button"
								class={`min-h-8 rounded px-3 text-xs font-medium ${portFamily === family ? 'bg-blue-600 text-white' : 'text-gray-700 dark:text-gray-300'}`}
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
							class="dark:border-dark-border flex min-h-10 cursor-pointer items-center gap-2 rounded-md border px-3 text-sm"
						>
							<input
								type="checkbox"
								checked={activePortSeries.has(`${option.side}-${option.range}`)}
								onchange={() => togglePortSeries(option.side, option.range)}
								class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
							/>
							<span class="text-gray-700 dark:text-gray-300">{option.label}</span>
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
