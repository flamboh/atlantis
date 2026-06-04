<script lang="ts">
	import { onMount } from 'svelte';
	import PrimaryFilters from '$lib/components/filters/PrimaryFilters.svelte';
	import NetflowDashboard from '$lib/components/netflow/NetflowDashboard.svelte';
	import IPChart from '$lib/components/charts/IPChart.svelte';
	import ProtocolChart from '$lib/components/charts/ProtocolChart.svelte';
	import SpectrumStatsChart from '$lib/components/charts/SpectrumStatsChart.svelte';
	import { DEFAULT_DATA_OPTIONS } from '$lib/components/netflow/constants';
	import type { DataOption, GroupByOption, RouterConfig } from '$lib/components/netflow/types.ts';
	import { clampGroupByToDateRange } from '$lib/components/charts/chart-utils';
	import {
		FLOW_SCOPE_OPTIONS,
		IP_METRIC_OPTIONS,
		type FlowScopeKey,
		type IpGranularity,
		type IpMetricKey,
		type ProtocolMetricKey
	} from '$lib/types/types';
	import { watch } from 'runed';
	import { useSearchParams } from 'runed/kit';
	import { createDateRangeSearchSchema } from '$lib/schemas';

	const props = $props<{
		dataset: string;
		defaultStartDate: string;
		routers?: string[];
		title?: string;
	}>();

	const params = (() =>
		useSearchParams(createDateRangeSearchSchema(props.defaultStartDate), {
			noScroll: true
		}))();
	let startDate = $state(params.startDate);
	let endDate = $state(params.endDate);
	let selectedGroupBy = $state<GroupByOption>(params.groupBy as GroupByOption);
	function createRouterConfig(routers: string[]): RouterConfig {
		const routerConfig: RouterConfig = {};
		for (const router of routers) {
			routerConfig[router] = true;
		}
		return routerConfig;
	}
	let selectedRouters = $state<RouterConfig>({});
	let selectedSpectrumRouter = $state('');
	let selectedSpectrumAddressType = $state<'sa' | 'da'>('sa');
	let flowScopeKey = $state<FlowScopeKey>('all');
	let dataOptions = $state<DataOption[]>(DEFAULT_DATA_OPTIONS.map((option) => ({ ...option })));
	const defaultIpMetrics: IpMetricKey[] = IP_METRIC_OPTIONS.slice(0, 2).map((option) => option.key);
	let ipMetrics = $state<IpMetricKey[]>([...defaultIpMetrics]);
	let protocolMetrics = $state<ProtocolMetricKey[]>(['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6']);
	type ChartCardId = 'dashboard' | 'ip' | 'protocol' | 'spectrum';
	const DEFAULT_CHART_ORDER: ChartCardId[] = ['dashboard', 'ip', 'protocol', 'spectrum'];
	const CHART_ORDER_STORAGE_KEY = 'netflow-main-chart-order-v3';
	let chartOrder = $state<ChartCardId[]>([...DEFAULT_CHART_ORDER]);
	let draggedChartId = $state<ChartCardId | null>(null);
	let dropTargetChartId = $state<ChartCardId | null>(null);
	let dragPreviewElement: HTMLElement | null = null;

	const GROUP_BY_TO_IP: Record<GroupByOption, IpGranularity> = {
		date: '1d',
		hour: '1h',
		'30min': '30m',
		'5min': '5m'
	};

	const ipGranularity = $derived(GROUP_BY_TO_IP[selectedGroupBy]);
	const flowScope = $derived(
		FLOW_SCOPE_OPTIONS.find((option) => option.key === flowScopeKey) ?? FLOW_SCOPE_OPTIONS[0]
	);
	const srcVisibility = $derived(flowScope.srcVisibility);
	const dstVisibility = $derived(flowScope.dstVisibility);
	const routers = $derived(Array.isArray(props.routers) ? props.routers : []);
	const routerStateKey = $derived(`${props.dataset}:${routers.join('\0')}`);
	const availableSpectrumRouters = $derived(getEnabledRouters(selectedRouters));
	const routersLoaded = $derived(Array.isArray(props.routers));

	function isValidChartOrder(value: unknown): value is ChartCardId[] {
		if (!Array.isArray(value)) {
			return false;
		}
		if (value.length !== DEFAULT_CHART_ORDER.length) {
			return false;
		}
		const order = new Set(value);
		return DEFAULT_CHART_ORDER.every((id) => order.has(id));
	}

	function loadChartOrder() {
		try {
			const raw = localStorage.getItem(CHART_ORDER_STORAGE_KEY);
			if (!raw) {
				return;
			}
			const parsed = JSON.parse(raw) as unknown;
			if (isValidChartOrder(parsed)) {
				chartOrder = parsed;
			}
		} catch (error) {
			console.error('Failed to load chart order', error);
		}
	}

	function persistChartOrder() {
		try {
			localStorage.setItem(CHART_ORDER_STORAGE_KEY, JSON.stringify(chartOrder));
		} catch (error) {
			console.error('Failed to save chart order', error);
		}
	}

	function moveChartCard(draggedId: ChartCardId, targetId: ChartCardId) {
		if (draggedId === targetId) {
			return;
		}
		const draggedIndex = chartOrder.indexOf(draggedId);
		const targetIndex = chartOrder.indexOf(targetId);
		if (draggedIndex === -1 || targetIndex === -1) {
			return;
		}
		const nextOrder = [...chartOrder];
		nextOrder.splice(draggedIndex, 1);
		nextOrder.splice(targetIndex, 0, draggedId);
		chartOrder = nextOrder;
	}

	function clearDragPreview() {
		if (dragPreviewElement) {
			dragPreviewElement.remove();
			dragPreviewElement = null;
		}
	}

	function handleChartDragStart(event: DragEvent, chartId: ChartCardId) {
		const target = event.target as HTMLElement | null;
		if (!target?.closest('[data-drag-handle]')) {
			event.preventDefault();
			return;
		}
		clearDragPreview();
		draggedChartId = chartId;
		dropTargetChartId = chartId;
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
			event.dataTransfer.setData('text/plain', chartId);

			const card = target?.closest('[data-chart-card]') as HTMLElement | null;
			if (card) {
				const rect = card.getBoundingClientRect();
				const clone = card.cloneNode(true) as HTMLElement;
				clone.style.position = 'fixed';
				clone.style.top = '-10000px';
				clone.style.left = '-10000px';
				clone.style.width = `${rect.width}px`;
				clone.style.height = `${rect.height}px`;
				clone.style.opacity = '1';
				clone.style.pointerEvents = 'none';
				clone.style.margin = '0';
				document.body.appendChild(clone);
				event.dataTransfer.setDragImage(
					clone,
					Math.max(0, event.clientX - rect.left),
					Math.max(0, event.clientY - rect.top)
				);
				dragPreviewElement = clone;
			}
		}
	}

	function handleChartDragOver(event: DragEvent, targetId: ChartCardId) {
		if (!draggedChartId || draggedChartId === targetId) {
			return;
		}
		event.preventDefault();
		dropTargetChartId = targetId;
		if (event.dataTransfer) {
			event.dataTransfer.dropEffect = 'move';
		}
	}

	function handleChartDragLeave(event: DragEvent, chartId: ChartCardId) {
		const currentTarget = event.currentTarget as HTMLElement | null;
		const relatedTarget = event.relatedTarget as Node | null;
		if (currentTarget && relatedTarget && currentTarget.contains(relatedTarget)) {
			return;
		}
		if (dropTargetChartId === chartId) {
			dropTargetChartId = null;
		}
	}

	function handleChartDrop(event: DragEvent, targetId: ChartCardId) {
		event.preventDefault();
		if (draggedChartId && targetId !== draggedChartId) {
			moveChartCard(draggedChartId, targetId);
			persistChartOrder();
		}
		draggedChartId = null;
		dropTargetChartId = null;
		clearDragPreview();
	}

	function handleChartDragEnd() {
		draggedChartId = null;
		dropTargetChartId = null;
		clearDragPreview();
	}

	function getEnabledRouters(routers: RouterConfig): string[] {
		return Object.entries(routers)
			.filter(([, enabled]) => enabled)
			.map(([router]) => router)
			.sort();
	}

	watch(
		() => params.startDate,
		(next) => {
			if (next !== startDate) {
				startDate = next;
			}
		}
	);

	watch(
		() => params.endDate,
		(next) => {
			if (next !== endDate) {
				endDate = next;
			}
		}
	);

	watch(
		() => params.groupBy,
		(next) => {
			const value = next as GroupByOption;
			if (value !== selectedGroupBy) {
				selectedGroupBy = value;
			}
		}
	);

	$effect(() => {
		const clampedGroupBy = clampGroupByToDateRange(selectedGroupBy, startDate, endDate);
		if (clampedGroupBy !== selectedGroupBy) {
			selectedGroupBy = clampedGroupBy;
			if (params.groupBy !== clampedGroupBy) {
				params.groupBy = clampedGroupBy;
			}
		}
	});

	onMount(() => {
		loadChartOrder();
	});

	let lastRouterStateKey = $state('');

	$effect(() => {
		const nextKey = routerStateKey;
		if (nextKey === lastRouterStateKey) {
			return;
		}
		lastRouterStateKey = nextKey;

		const nextRouters = routers;
		const nextSelectedRouters = createRouterConfig(nextRouters);
		selectedRouters = nextSelectedRouters;

		const enabledRouters = getEnabledRouters(nextSelectedRouters);
		selectedSpectrumRouter = nextRouters[0] ?? enabledRouters[0] ?? '';
	});

	$effect(() => {
		if (!availableSpectrumRouters.includes(selectedSpectrumRouter)) {
			selectedSpectrumRouter = availableSpectrumRouters[0] ?? '';
		}
	});

	function handleStartDateChange(event: CustomEvent<{ startDate: string }>) {
		startDate = event.detail.startDate;
		params.startDate = startDate;
	}

	function handleEndDateChange(event: CustomEvent<{ endDate: string }>) {
		endDate = event.detail.endDate;
		params.endDate = endDate;
	}

	function handleDateChange(event: CustomEvent<{ startDate: string; endDate: string }>) {
		startDate = event.detail.startDate;
		endDate = event.detail.endDate;
		params.update({ startDate, endDate });
	}

	function handleGroupByChange(event: CustomEvent<{ groupBy: GroupByOption }>) {
		if (event.detail.groupBy === params.groupBy) {
			return;
		}
		selectedGroupBy = event.detail.groupBy;
		params.groupBy = selectedGroupBy;
	}

	function handleRoutersChange(event: CustomEvent<{ routers: RouterConfig }>) {
		const nextRouters = event.detail.routers;
		selectedRouters = nextRouters;
		const enabledRouters = getEnabledRouters(nextRouters);
		if (!enabledRouters.includes(selectedSpectrumRouter)) {
			selectedSpectrumRouter = enabledRouters[0] ?? '';
		}
	}

	function handleDataOptionsChange(event: CustomEvent<{ options: DataOption[] }>) {
		dataOptions = event.detail.options;
	}

	function handleIpMetricsChange(event: CustomEvent<{ metrics: IpMetricKey[] }>) {
		ipMetrics = event.detail.metrics;
	}

	function handleScopeChange(event: CustomEvent<{ scope: FlowScopeKey }>) {
		flowScopeKey = event.detail.scope;
	}

	function handleResetView() {
		const today = new Date().toJSON().slice(0, 10);
		selectedGroupBy = 'date';
		startDate = props.defaultStartDate;
		endDate = today;
		params.update({ groupBy: selectedGroupBy, startDate, endDate });
	}
</script>

<svelte:head>
	<title>{props.title ?? `ATLANTIS - ${props.dataset}`}</title>
	<meta name="description" content="NetFlow analysis and visualization tool" />
</svelte:head>

<main class="mx-auto flex max-w-[95vw] flex-col gap-2 px-4 py-4 sm:px-2 lg:px-4">
	<h1 class="px-1 text-2xl font-semibold text-gray-900 dark:text-gray-100">
		{props.title ?? props.dataset}
	</h1>

	<PrimaryFilters
		{startDate}
		{endDate}
		groupBy={selectedGroupBy}
		routers={selectedRouters}
		flowScope={flowScopeKey}
		on:startDateChange={handleStartDateChange}
		on:endDateChange={handleEndDateChange}
		on:groupByChange={handleGroupByChange}
		on:routersChange={handleRoutersChange}
		on:scopeChange={handleScopeChange}
		on:resetView={handleResetView}
	/>

	<div role="list" aria-label="Reorderable charts" class="flex flex-col gap-2">
		{#each chartOrder as chartId (chartId)}
			<section
				role="listitem"
				data-chart-card
				class={`rounded-lg ${dropTargetChartId === chartId && draggedChartId && draggedChartId !== chartId ? 'ring-2 ring-blue-400 ring-offset-2' : ''}`}
				ondragstart={(event) => {
					handleChartDragStart(event, chartId);
				}}
				ondragend={handleChartDragEnd}
				ondragover={(event) => {
					handleChartDragOver(event, chartId);
				}}
				ondragleave={(event) => {
					handleChartDragLeave(event, chartId);
				}}
				ondrop={(event) => {
					handleChartDrop(event, chartId);
				}}
			>
				{#if chartId === 'dashboard'}
					<NetflowDashboard
						dataset={props.dataset}
						{startDate}
						{endDate}
						groupBy={selectedGroupBy}
						routers={selectedRouters}
						{routersLoaded}
						{dataOptions}
						{srcVisibility}
						{dstVisibility}
						on:dateChange={handleDateChange}
						on:groupByChange={handleGroupByChange}
						on:dataOptionsChange={handleDataOptionsChange}
					/>
				{:else if chartId === 'ip'}
					<IPChart
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						routers={selectedRouters}
						activeMetrics={ipMetrics}
						{srcVisibility}
						{dstVisibility}
						on:dateChange={handleDateChange}
						on:groupByChange={handleGroupByChange}
						on:metricsChange={handleIpMetricsChange}
					/>
				{:else if chartId === 'protocol'}
					<ProtocolChart
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						routers={selectedRouters}
						activeMetrics={protocolMetrics}
						{srcVisibility}
						{dstVisibility}
						on:dateChange={handleDateChange}
						on:groupByChange={handleGroupByChange}
						on:metricsChange={(event) => {
							protocolMetrics = event.detail.metrics;
						}}
					/>
				{:else}
					<SpectrumStatsChart
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						router={selectedSpectrumRouter}
						addressType={selectedSpectrumAddressType}
						availableRouters={availableSpectrumRouters}
						{srcVisibility}
						{dstVisibility}
						on:dateChange={handleDateChange}
						on:groupByChange={handleGroupByChange}
						on:routerChange={(event) => {
							selectedSpectrumRouter = event.detail.router;
						}}
						on:addressTypeChange={(event) => {
							selectedSpectrumAddressType = event.detail.addressType;
						}}
					/>
				{/if}
			</section>
		{/each}
	</div>
</main>
