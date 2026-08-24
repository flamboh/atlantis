<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import DatasetTabs from '$lib/components/datasets/DatasetTabs.svelte';
	import PrimaryFilters from '$lib/components/filters/PrimaryFilters.svelte';
	import NetflowDashboard from '$lib/components/netflow/NetflowDashboard.svelte';
	import BreakdownChart from '$lib/components/charts/BreakdownChart.svelte';
	import FlowCharacteristicsChart from '$lib/components/charts/FlowCharacteristicsChart.svelte';
	import PortCardinalityChart from '$lib/components/charts/PortCardinalityChart.svelte';
	import CoverageStrip from '$lib/components/charts/CoverageStrip.svelte';
	import { createFlowCharacteristicsData } from '$lib/components/charts/flow-characteristics-data.svelte';
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import { DEFAULT_DATA_OPTIONS } from '$lib/components/netflow/constants';
	import { createNearViewportAttachment } from '$lib/components/netflow/near-viewport';
	import type { DataOption, GroupByOption, RouterConfig } from '$lib/components/netflow/types.ts';
	import type { Attachment } from 'svelte/attachments';
	import { clampGroupByToDateRange } from '$lib/components/charts/chart-utils';
	import {
		FLOW_SCOPE_OPTIONS,
		type FlowVisibility,
		IP_METRIC_OPTIONS,
		type FlowScopeKey,
		type IpGranularity,
		type IpMetricKey,
		type ProtocolMetricKey
	} from '$lib/types/types';
	import { watch } from 'runed';
	import { useSearchParams } from 'runed/kit';
	import { createDateRangeSearchSchema } from '$lib/schemas';
	import { navigateToNetflowFile } from '$lib/utils/netflow-file-navigation';

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
	let dataOptions = $state<DataOption[]>(DEFAULT_DATA_OPTIONS.map((option) => ({ ...option })));
	const defaultIpMetrics: IpMetricKey[] = IP_METRIC_OPTIONS.slice(0, 2).map((option) => option.key);
	let ipMetrics = $state<IpMetricKey[]>([...defaultIpMetrics]);
	let protocolMetrics = $state<ProtocolMetricKey[]>(['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6']);
	type ChartCardId =
		| 'dashboard'
		| 'characteristics'
		| 'ports'
		| 'ip'
		| 'protocol'
		| 'spectrum'
		| 'coverage';
	const DEFAULT_CHART_ORDER: ChartCardId[] = [
		'dashboard',
		'characteristics',
		'ports',
		'ip',
		'protocol',
		'spectrum',
		'coverage'
	];
	const CHART_CARD_DETAILS: Record<ChartCardId, { title: string; minimumHeight: number }> = {
		dashboard: { title: 'Traffic Overview', minimumHeight: 640 },
		characteristics: { title: 'Flow Characteristics', minimumHeight: 440 },
		ports: { title: 'Unique Ports', minimumHeight: 440 },
		ip: { title: 'IP Address Breakdown', minimumHeight: 440 },
		protocol: { title: 'Protocol Breakdown', minimumHeight: 440 },
		spectrum: { title: 'IP Address Spectrum', minimumHeight: 560 },
		coverage: { title: 'Coverage', minimumHeight: 113 }
	};
	const CHART_ORDER_STORAGE_KEY = 'netflow-main-chart-order-v5';
	let chartOrder = $state<ChartCardId[]>([...DEFAULT_CHART_ORDER]);
	let activatedCharts = $state<Record<ChartCardId, boolean>>({
		dashboard: false,
		characteristics: false,
		ports: false,
		ip: false,
		protocol: false,
		spectrum: false,
		coverage: false
	});
	let draggedChartId = $state<ChartCardId | null>(null);
	let dropTargetChartId = $state<ChartCardId | null>(null);
	let dragPreviewElement: HTMLElement | null = null;
	const chartVisibilityAttachments: Record<ChartCardId, Attachment<HTMLElement>> = {
		dashboard: createNearViewportAttachment(() => {
			activatedCharts.dashboard = true;
		}),
		characteristics: createNearViewportAttachment(() => {
			activatedCharts.characteristics = true;
		}),
		ports: createNearViewportAttachment(() => {
			activatedCharts.ports = true;
		}),
		ip: createNearViewportAttachment(() => {
			activatedCharts.ip = true;
		}),
		protocol: createNearViewportAttachment(() => {
			activatedCharts.protocol = true;
		}),
		spectrum: createNearViewportAttachment(() => {
			activatedCharts.spectrum = true;
		}),
		coverage: createNearViewportAttachment(() => {
			activatedCharts.coverage = true;
		})
	};

	function activateChart(chartId: ChartCardId) {
		activatedCharts[chartId] = true;
	}

	function getCardMinimumHeight(chartId: ChartCardId): number {
		if (chartId !== 'coverage') {
			return CHART_CARD_DETAILS[chartId].minimumHeight;
		}
		const coverageCanvasHeight = Math.max(48, availableSpectrumRouters.length * 18 + 30);
		return coverageCanvasHeight + 65;
	}

	const GROUP_BY_TO_IP: Record<GroupByOption, IpGranularity> = {
		date: '1d',
		hour: '1h',
		'30min': '30m',
		'5min': '5m'
	};

	const ipGranularity = $derived(GROUP_BY_TO_IP[selectedGroupBy]);
	function getFlowScopeKey(
		srcVisibility: FlowVisibility,
		dstVisibility: FlowVisibility
	): FlowScopeKey {
		return (
			FLOW_SCOPE_OPTIONS.find(
				(option) => option.srcVisibility === srcVisibility && option.dstVisibility === dstVisibility
			)?.key ?? 'all'
		);
	}

	const flowScopeKey = $derived(
		getFlowScopeKey(params.srcVisibility as FlowVisibility, params.dstVisibility as FlowVisibility)
	);
	const srcVisibility = $derived(params.srcVisibility as FlowVisibility);
	const dstVisibility = $derived(params.dstVisibility as FlowVisibility);
	const routers = $derived(Array.isArray(props.routers) ? props.routers : []);
	const routerStateKey = $derived(`${props.dataset}:${routers.join('\0')}`);
	const availableSpectrumRouters = $derived(getEnabledRouters(selectedRouters));
	const routersLoaded = $derived(Array.isArray(props.routers));
	const flowCharacteristics = createFlowCharacteristicsData(() => ({
		enabled: activatedCharts.characteristics || activatedCharts.ports,
		dataset: props.dataset,
		startDate,
		endDate,
		groupBy: selectedGroupBy,
		routers: selectedRouters,
		routersLoaded,
		srcVisibility,
		dstVisibility
	}));

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
		activateChart(chartOrder[0] ?? 'dashboard');
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

	function handleStartDateChange(payload: { startDate: string }) {
		startDate = payload.startDate;
		params.startDate = startDate;
	}

	function handleEndDateChange(payload: { endDate: string }) {
		endDate = payload.endDate;
		params.endDate = endDate;
	}

	function handleDateChange(payload: { startDate: string; endDate: string }) {
		startDate = payload.startDate;
		endDate = payload.endDate;
		params.update({ startDate, endDate });
	}

	function handleGroupByChange(payload: { groupBy: GroupByOption }) {
		if (payload.groupBy === params.groupBy) {
			return;
		}
		selectedGroupBy = payload.groupBy;
		params.groupBy = selectedGroupBy;
	}

	function handleMetricDrillDown(
		groupBy: GroupByOption,
		nextStartDate: string,
		nextEndDate: string
	) {
		handleGroupByChange({ groupBy });
		handleDateChange({ startDate: nextStartDate, endDate: nextEndDate });
	}

	function handleMetricNavigateToFile(slug: string) {
		void navigateToNetflowFile(goto, slug, props.dataset, { srcVisibility, dstVisibility });
	}

	function handleRoutersChange(payload: { routers: RouterConfig }) {
		const nextRouters = payload.routers;
		selectedRouters = nextRouters;
		const enabledRouters = getEnabledRouters(nextRouters);
		if (!enabledRouters.includes(selectedSpectrumRouter)) {
			selectedSpectrumRouter = enabledRouters[0] ?? '';
		}
	}

	function handleDataOptionsChange(payload: { options: DataOption[] }) {
		dataOptions = payload.options;
	}

	function handleIpMetricsChange(payload: { metrics: IpMetricKey[] }) {
		ipMetrics = payload.metrics;
	}

	function handleScopeChange(payload: { scope: FlowScopeKey }) {
		const flowScope = FLOW_SCOPE_OPTIONS.find((option) => option.key === payload.scope);
		if (!flowScope) {
			return;
		}
		params.update({
			srcVisibility: flowScope.srcVisibility,
			dstVisibility: flowScope.dstVisibility
		});
	}

	function handleResetView() {
		const today = new Date().toJSON().slice(0, 10);
		selectedGroupBy = 'date';
		startDate = props.defaultStartDate;
		endDate = today;
		params.update({
			groupBy: selectedGroupBy,
			startDate,
			endDate,
			srcVisibility: 'all',
			dstVisibility: 'all'
		});
	}
</script>

<svelte:head>
	<title>{props.title ?? `ATLANTIS - ${props.dataset}`}</title>
	<meta name="description" content="NetFlow analysis and visualization tool" />
</svelte:head>

<main class="mx-auto flex max-w-[95vw] flex-col gap-2 px-4 py-4 sm:px-2 lg:px-4">
	<h1 class="text-foreground px-1 text-2xl font-semibold">
		{props.title ?? props.dataset}
	</h1>
	<DatasetTabs datasetId={props.dataset} active="dashboard" />

	<PrimaryFilters
		{startDate}
		{endDate}
		groupBy={selectedGroupBy}
		routers={selectedRouters}
		flowScope={flowScopeKey}
		onStartDateChange={handleStartDateChange}
		onEndDateChange={handleEndDateChange}
		onGroupByChange={handleGroupByChange}
		onRoutersChange={handleRoutersChange}
		onScopeChange={handleScopeChange}
		onResetView={handleResetView}
	/>
	<div role="list" aria-label="Reorderable charts" class="flex flex-col gap-2">
		{#each chartOrder as chartId (chartId)}
			<section
				role="listitem"
				data-chart-card
				data-chart-id={chartId}
				data-chart-activated={activatedCharts[chartId]}
				class={`rounded-lg ${dropTargetChartId === chartId && draggedChartId && draggedChartId !== chartId ? 'ring-primary ring-offset-background ring-2 ring-offset-2' : ''}`}
				style={`min-height:${getCardMinimumHeight(chartId)}px`}
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
				{#if !activatedCharts[chartId]}
					<div
						class="border-border bg-card text-card-foreground relative flex h-full flex-col rounded-lg border shadow-sm"
						style={`min-height:${getCardMinimumHeight(chartId)}px`}
						data-testid={`deferred-chart-${chartId}`}
					>
						<div
							class="border-border relative cursor-grab border-b p-4 select-none active:cursor-grabbing"
							draggable="true"
							data-drag-handle
						>
							<h2 class="text-lg font-semibold">{CHART_CARD_DETAILS[chartId].title}</h2>
							<DragGrip />
						</div>
						<div
							{@attach chartVisibilityAttachments[chartId]}
							class="pointer-events-none h-px w-full"
							data-chart-sentinel={chartId}
							aria-hidden="true"
						></div>
						<div
							class="text-muted-foreground flex flex-1 flex-col items-center justify-center gap-3 p-4 text-sm"
						>
							<p>This chart will load as it approaches the viewport.</p>
							<button
								type="button"
								class="border-input bg-background hover:bg-accent hover:text-accent-foreground focus-visible:ring-ring rounded-md border px-3 py-2 text-sm font-medium focus-visible:ring-2 focus-visible:outline-none"
								onclick={() => activateChart(chartId)}
							>
								Load {CHART_CARD_DETAILS[chartId].title} chart
							</button>
						</div>
					</div>
				{:else if chartId === 'dashboard'}
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
						onDateChange={handleDateChange}
						onGroupByChange={handleGroupByChange}
						onDataOptionsChange={handleDataOptionsChange}
					/>
				{:else if chartId === 'characteristics'}
					<FlowCharacteristicsChart
						data={flowCharacteristics.data}
						loading={flowCharacteristics.loading}
						error={flowCharacteristics.error}
						groupBy={selectedGroupBy}
						onDrillDown={handleMetricDrillDown}
						onNavigateToFile={handleMetricNavigateToFile}
					/>
				{:else if chartId === 'ports'}
					<PortCardinalityChart
						data={flowCharacteristics.data}
						loading={flowCharacteristics.loading}
						error={flowCharacteristics.error}
						groupBy={selectedGroupBy}
						onDrillDown={handleMetricDrillDown}
						onNavigateToFile={handleMetricNavigateToFile}
					/>
				{:else if chartId === 'ip'}
					<BreakdownChart
						kind="ip"
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						routers={selectedRouters}
						activeMetrics={ipMetrics}
						{srcVisibility}
						{dstVisibility}
						onDateChange={handleDateChange}
						onGroupByChange={handleGroupByChange}
						onMetricsChange={handleIpMetricsChange}
					/>
				{:else if chartId === 'protocol'}
					<BreakdownChart
						kind="protocol"
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						routers={selectedRouters}
						activeMetrics={protocolMetrics}
						{srcVisibility}
						{dstVisibility}
						onDateChange={handleDateChange}
						onGroupByChange={handleGroupByChange}
						onMetricsChange={(payload) => {
							protocolMetrics = payload.metrics;
						}}
					/>
				{:else if chartId === 'spectrum'}
					<BreakdownChart
						kind="spectrum"
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						router={selectedSpectrumRouter}
						addressType={selectedSpectrumAddressType}
						availableRouters={availableSpectrumRouters}
						{srcVisibility}
						{dstVisibility}
						onDateChange={handleDateChange}
						onGroupByChange={handleGroupByChange}
						onRouterChange={(payload) => {
							selectedSpectrumRouter = payload.router;
						}}
						onAddressTypeChange={(payload) => {
							selectedSpectrumAddressType = payload.addressType;
						}}
					/>
				{:else}
					<CoverageStrip
						dataset={props.dataset}
						{startDate}
						{endDate}
						groupBy={selectedGroupBy}
						routers={selectedRouters}
						{routersLoaded}
					/>
				{/if}
			</section>
		{/each}
	</div>
</main>
