<script lang="ts">
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import { createEventDispatcher } from 'svelte';
	import { goto } from '$app/navigation';
	import ChartContainer from '$lib/components/charts/ChartContainer.svelte';
	import MetricSelector from '$lib/components/filters/MetricSelector.svelte';
	import { dateStringToEpochPST } from '$lib/utils/timezone';
	import { navigateToNetflowFile } from '$lib/utils/netflow-file-navigation';
	import {
		ensureCachedWindow,
		getMissingWindowRanges,
		readCachedWindow,
		type TimeRange
	} from '$lib/utils/window-cache';
	import type {
		DataOption,
		GroupByOption,
		NetflowDataPoint,
		ChartTypeOption,
		RouterConfig
	} from './types.ts';
	import type {
		NetflowIpFamily,
		NetflowMetricTotals,
		NetflowStatsResponse,
		NetflowStatsResult
	} from '$lib/types/types';

	const props = $props<{
		dataset: string;
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		routersLoaded: boolean;
		dataOptions: DataOption[];
	}>();

	const dispatch = createEventDispatcher<{
		dateChange: { startDate: string; endDate: string };
		groupByChange: { groupBy: GroupByOption };
		dataOptionsChange: { options: DataOption[] };
	}>();
	const IP_FAMILY_LABELS: Record<NetflowIpFamily, string> = {
		all: 'All',
		ipv4: 'IPv4',
		ipv6: 'IPv6'
	};

	let chartType = $state<ChartTypeOption>('stacked');
	let selectedIpFamily = $state<NetflowIpFamily>('all');
	let availableIpFamilies = $state<NetflowIpFamily[]>(['all']);
	let rawResults = $state.raw<NetflowStatsResult[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

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

	let results = $derived.by<NetflowDataPoint[]>(() => {
		const suffix =
			selectedIpFamily === 'all' ? null : selectedIpFamily === 'ipv4' ? 'Ipv4' : 'Ipv6';
		return rawResults.map((row) => ({
			bucketStart: row.bucketStart,
			...getMetricsForFamily(row, suffix)
		}));
	});

	let ipFamilyOptions = $derived.by(() =>
		availableIpFamilies.length > 1
			? availableIpFamilies.map((family) => ({
					value: family,
					label: IP_FAMILY_LABELS[family]
				}))
			: []
	);

	type FilterInputs = {
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
	};

	let lastFiltersKey = '';
	let requestToken = 0;
	const ipFamilyCache = new Map<string, NetflowIpFamily[]>();

	function setAvailableIpFamilies(ipFamilies: NetflowIpFamily[]) {
		availableIpFamilies = ipFamilies;
		if (!ipFamilies.includes(selectedIpFamily)) {
			selectedIpFamily = 'all';
		}
	}

	function deriveSelectedRouters(routers: RouterConfig): string[] {
		return Object.entries(routers)
			.filter(([, enabled]) => enabled)
			.map(([router]) => router.trim())
			.filter((router) => router.length > 0)
			.sort();
	}

	function getCacheKey(filters: FilterInputs, selectedRouters: string[]): string {
		return JSON.stringify({
			chart: 'netflow',
			dataset: props.dataset,
			groupBy: filters.groupBy,
			routers: selectedRouters
		});
	}

	function getRequestedRange(filters: FilterInputs): TimeRange {
		return {
			start: dateStringToEpochPST(filters.startDate),
			end: dateStringToEpochPST(filters.endDate, true)
		};
	}

	function readCachedResults(cacheKey: string, requestedRange: TimeRange): NetflowStatsResult[] {
		return readCachedWindow<NetflowStatsResult>(cacheKey, requestedRange, (record, range) => {
			return record.bucketStart >= range.start && record.bucketStart < range.end;
		});
	}

	function handleIpFamilyChange(ipFamily: NetflowIpFamily) {
		selectedIpFamily = ipFamily;
	}

	async function loadData(
		filters: FilterInputs,
		token: number,
		selectedRouters: string[],
		requestedRange: TimeRange
	) {
		const cacheKey = getCacheKey(filters, selectedRouters);
		const needsFetch = getMissingWindowRanges(cacheKey, requestedRange).length > 0;
		loading = needsFetch;
		error = null;

		const params = new URLSearchParams({
			dataset: props.dataset,
			routers: selectedRouters.join(','),
			groupBy: filters.groupBy
		});

		try {
			await ensureCachedWindow<NetflowStatsResult>({
				key: cacheKey,
				requestedRange,
				fetchRange: async (range) => {
					const response = await fetch(
						`/api/netflow/stats?${new URLSearchParams({
							...Object.fromEntries(params.entries()),
							startDate: range.start.toString(),
							endDate: range.end.toString()
						}).toString()}`,
						{
							method: 'GET',
							headers: {
								'Content-Type': 'application/json'
							}
						}
					);

					if (!response.ok) {
						const message = await response.text();
						throw new Error(message || `Failed to load data: ${response.statusText}`);
					}

					const json = (await response.json()) as NetflowStatsResponse;
					ipFamilyCache.set(cacheKey, json.availableIpFamilies);
					return json.result;
				},
				getRecordKey: (record) => `${record.bucketStart}`,
				compareRecords: (left, right) => left.bucketStart - right.bucketStart
			});

			if (token !== requestToken) {
				return;
			}
			setAvailableIpFamilies(ipFamilyCache.get(cacheKey) ?? ['all']);
			rawResults = readCachedResults(cacheKey, requestedRange);
		} catch (err) {
			if (token !== requestToken) {
				return;
			}
			error = `Failed to load data: ${err instanceof Error ? err.message : 'Unknown error'}`;
			rawResults = [];
		} finally {
			if (token === requestToken) {
				loading = false;
			}
		}
	}

	function handleDrillDown(newGroupBy: GroupByOption, newStartDate: string, newEndDate: string) {
		dispatch('groupByChange', { groupBy: newGroupBy });
		dispatch('dateChange', { startDate: newStartDate, endDate: newEndDate });
	}

	function handleNavigateToFile(slug: string) {
		void navigateToNetflowFile(goto, slug, props.dataset);
	}

	function handleChartTypeChange(newChartType: ChartTypeOption) {
		chartType = newChartType;
	}

	function handleDataOptionsChange(nextOptions: DataOption[]) {
		dispatch('dataOptionsChange', { options: nextOptions });
	}

	$effect(() => {
		const filters: FilterInputs = {
			startDate: props.startDate,
			endDate: props.endDate,
			groupBy: props.groupBy,
			routers: props.routers
		};

		const selectedRouters = deriveSelectedRouters(filters.routers);

		if (!props.routersLoaded) {
			error = null;
			rawResults = [];
			loading = true;
			return;
		}

		if (selectedRouters.length === 0) {
			error = 'Select at least one source to view NetFlow statistics';
			rawResults = [];
			loading = false;
			return;
		}

		const nextKey = JSON.stringify({
			dataset: props.dataset,
			startDate: filters.startDate,
			endDate: filters.endDate,
			groupBy: filters.groupBy,
			routers: selectedRouters
		});

		if (nextKey === lastFiltersKey) {
			return;
		}

		lastFiltersKey = nextKey;
		const requestedRange = getRequestedRange(filters);
		const token = ++requestToken;
		loadData(filters, token, selectedRouters, requestedRange);
	});
</script>

<div
	class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white shadow-sm dark:shadow-none"
>
	<div
		class="dark:border-dark-border relative cursor-grab border-b p-4 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<h2 class="text-lg font-semibold text-gray-900 dark:text-gray-100">Traffic Overview</h2>
		<DragGrip />
	</div>

	<div class="space-y-4 p-4">
		<MetricSelector
			dataOptions={props.dataOptions}
			onDataOptionsChange={handleDataOptionsChange}
			{chartType}
			onChartTypeChange={handleChartTypeChange}
			{ipFamilyOptions}
			{selectedIpFamily}
			onIpFamilyChange={handleIpFamilyChange}
		/>

		<div
			class="dark:border-dark-border dark:bg-dark-subtle/60 h-[380px] min-h-[280px] resize-none overflow-hidden rounded-md border border-gray-200 bg-white/60 md:h-[320px] md:min-h-[240px] md:resize-y md:overflow-auto"
		>
			{#if loading}
				<div class="flex h-full items-center justify-center">
					<div class="flex items-center gap-3 text-gray-500 dark:text-gray-400">
						<div
							class="dark:border-dark-border h-5 w-5 animate-spin rounded-full border-2 border-gray-300 border-t-gray-500 dark:border-t-gray-400"
							aria-hidden="true"
						></div>
						<div>Loading data...</div>
					</div>
				</div>
			{:else if error}
				<div class="flex h-full items-center justify-center">
					<div class="text-red-500">{error}</div>
				</div>
			{:else if results.length === 0}
				<div class="flex h-full items-center justify-center">
					<div class="text-gray-500 dark:text-gray-400">
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
	</div>
</div>
