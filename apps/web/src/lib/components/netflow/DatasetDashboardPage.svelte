<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount, untrack } from 'svelte';
	import DashboardToolbar from '#lib/components/filters/DashboardToolbar.svelte';
	import ChartCardHeader from '#lib/components/charts/ChartCardHeader.svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Card from '#lib/components/ui/card/index.ts';
	import DashboardCardSlot from '#lib/components/netflow/DashboardCardSlot.svelte';
	import KpiRow from '#lib/components/netflow/KpiRow.svelte';
	import { createNetflowStatsData } from '#lib/components/netflow/netflow-stats-data.svelte.ts';
	import NetflowDashboard from '#lib/components/netflow/NetflowDashboard.svelte';
	import BreakdownChart from '#lib/components/charts/BreakdownChart.svelte';
	import FlowCharacteristicsChart from '#lib/components/charts/FlowCharacteristicsChart.svelte';
	import PortCardinalityChart from '#lib/components/charts/PortCardinalityChart.svelte';
	import CoverageStrip from '#lib/components/charts/CoverageStrip.svelte';
	import { createFlowCharacteristicsData } from '#lib/components/charts/flow-characteristics-data.svelte.ts';
	import { DEFAULT_DATA_OPTIONS } from '#lib/components/netflow/constants.ts';
	import { createNearViewportAttachment } from '#lib/components/netflow/near-viewport.ts';
	import type { DataOption, GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
	import type { Attachment } from 'svelte/attachments';
	import { clampGroupByToDateRange } from '#lib/components/charts/chart-utils.ts';
	import {
		type FlowDirection,
		IP_METRIC_OPTIONS,
		type IpGranularity,
		type IpMetricKey,
		MAAD_ADDRESS_SIDES,
		maadInternalSideCopy,
		type MaadIpVersion,
		type MaadMeasure,
		maadMeasureHasSpectrum,
		maadSideComputed,
		type ProtocolMetricKey
	} from '#lib/types/types.ts';
	import type { DimensionSide } from '#lib/components/charts/breakdown-chart-config.ts';
	import type { DimensionMetricKey } from '#lib/types/dimension-stats.ts';
	import { createDateRangeSearch, type DateRangeSearch } from '#lib/schemas.ts';
	import { navigateToNetflowFile } from '#lib/utils/netflow-file-navigation.ts';
	import { navigateToSearchParams } from '#lib/utils/search-navigation.ts';

	const props = $props<{
		dataset: string;
		defaultStartDate: string;
		hasLocality?: boolean;
		maadComputed?: boolean;
		maadInternalSide?: boolean;
		routers?: string[];
		title?: string;
	}>();

	const dateRangeSearch = $derived(createDateRangeSearch(props.defaultStartDate));
	let search = $derived(dateRangeSearch.parse(page.url.searchParams));
	const startDate = $derived(search.startDate);
	const endDate = $derived(search.endDate);
	const selectedGroupBy = $derived(clampGroupByToDateRange(search.groupBy, startDate, endDate));

	function updateSearch(patch: Partial<DateRangeSearch>) {
		const next = { ...search, ...patch };
		next.groupBy = clampGroupByToDateRange(next.groupBy, next.startDate, next.endDate);
		if (dateRangeSearch.equals(search, next)) return;
		search = next;
		void navigateToSearchParams(goto, dateRangeSearch.serialize(page.url.searchParams, next));
	}

	function createRouterConfig(routers: string[]): RouterConfig {
		const routerConfig: RouterConfig = {};
		for (const router of routers) {
			routerConfig[router] = true;
		}
		return routerConfig;
	}
	let requestedSpectrumRouter = $state('');
	let selectedSpectrumAddressType = $state<'sa' | 'da'>('sa');
	let dataOptions = $state<DataOption[]>(DEFAULT_DATA_OPTIONS.map((option) => ({ ...option })));
	const defaultIpMetrics: IpMetricKey[] = IP_METRIC_OPTIONS.slice(0, 2).map((option) => option.key);
	let ipMetrics = $state<IpMetricKey[]>([...defaultIpMetrics]);
	let protocolMetrics = $state<ProtocolMetricKey[]>(['uniqueProtocolsIpv4', 'uniqueProtocolsIpv6']);
	let dimensionMetrics = $state<DimensionMetricKey[]>(['saD1']);
	type ChartCardId =
		| 'dashboard'
		| 'characteristics'
		| 'ports'
		| 'ip'
		| 'protocol'
		| 'dimensions'
		| 'spectrum'
		| 'coverage';
	const DEFAULT_CHART_ORDER: ChartCardId[] = [
		'dashboard',
		'characteristics',
		'ports',
		'ip',
		'protocol',
		'dimensions',
		'spectrum',
		'coverage'
	];
	const CHART_CARD_DETAILS: Record<ChartCardId, { title: string; minimumHeight: number }> = {
		dashboard: { title: 'Traffic Overview', minimumHeight: 571 },
		characteristics: { title: 'Flow Characteristics', minimumHeight: 691 },
		ports: { title: 'Unique Ports', minimumHeight: 371 },
		ip: { title: 'Unique IP Counts', minimumHeight: 371 },
		protocol: { title: 'Unique Protocol Counts', minimumHeight: 371 },
		dimensions: { title: 'MAAD Dimensions', minimumHeight: 371 },
		spectrum: { title: 'Spectrum', minimumHeight: 451 },
		coverage: { title: 'Coverage', minimumHeight: 113 }
	};
	const UNAVAILABLE_CARD_MINIMUM_HEIGHT = 214;
	const CHART_ORDER_STORAGE_KEY = 'netflow-main-chart-order-v6';
	let chartOrder = $state<ChartCardId[]>([...DEFAULT_CHART_ORDER]);
	let activatedCharts = $state<Record<ChartCardId, boolean>>({
		dashboard: false,
		characteristics: false,
		ports: false,
		ip: false,
		protocol: false,
		dimensions: false,
		spectrum: false,
		coverage: false
	});
	let chartHeights = $state<Partial<Record<ChartCardId, number>>>({});
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
		dimensions: createNearViewportAttachment(() => {
			activatedCharts.dimensions = true;
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
		if (
			(chartId === 'dimensions' && maadUnavailableCopy) ||
			(chartId === 'spectrum' && spectrumUnavailableCopy)
		) {
			return UNAVAILABLE_CARD_MINIMUM_HEIGHT;
		}
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
		'10min': '10m',
		'5min': '5m'
	};

	const ipGranularity = $derived(GROUP_BY_TO_IP[selectedGroupBy]);
	const hasLocality = $derived(props.hasLocality ?? true);
	const ipVersion = $derived<MaadIpVersion>(search.ipVersion);
	const direction = $derived<FlowDirection>(hasLocality ? search.direction : 'all');
	const measure = $derived<MaadMeasure>(search.measure);
	const maadComputed = $derived(props.maadComputed ?? true);
	const maadSkippedSides = $derived(
		MAAD_ADDRESS_SIDES.filter(
			(side) => !maadSideComputed(direction, side, props.maadInternalSide ?? true)
		)
	);
	const maadUnavailableCopy = $derived(
		!maadComputed
			? 'MAAD was not computed for this dataset. Rebuild it without --no-maad to chart MAAD results.'
			: maadSkippedSides.length === MAAD_ADDRESS_SIDES.length
				? maadInternalSideCopy(direction, maadSkippedSides)
				: null
	);
	const maadSideUnavailableCopy = $derived<Partial<Record<DimensionSide, string>>>(
		Object.fromEntries(
			maadSkippedSides.map((side) => [
				side === 'source' ? 'sa' : 'da',
				maadInternalSideCopy(direction, [side])
			])
		)
	);
	const spectrumUnavailableCopy = $derived(
		maadUnavailableCopy ??
			(maadMeasureHasSpectrum(measure)
				? null
				: 'The spectrum is only computed for the Addresses measure. Set MAAD to Addresses in the toolbar to see it.')
	);
	const routers = $derived(Array.isArray(props.routers) ? props.routers : []);
	const routerStateKey = $derived(`${props.dataset}:${routers.join('\0')}`);
	let selectedRouters = $derived.by(() => {
		void routerStateKey;
		return createRouterConfig(untrack(() => routers));
	});
	const availableSpectrumRouters = $derived(getEnabledRouters(selectedRouters));
	const selectedSpectrumRouter = $derived(
		availableSpectrumRouters.includes(requestedSpectrumRouter)
			? requestedSpectrumRouter
			: (availableSpectrumRouters[0] ?? '')
	);
	const routersLoaded = $derived(Array.isArray(props.routers));
	const today = new Date().toJSON().slice(0, 10);
	const netflowStats = createNetflowStatsData(() => ({
		dataset: props.dataset,
		startDate,
		endDate,
		groupBy: selectedGroupBy,
		routers: selectedRouters,
		routersLoaded,
		direction
	}));
	const flowCharacteristics = createFlowCharacteristicsData(() => ({
		enabled: activatedCharts.characteristics || activatedCharts.ports,
		dataset: props.dataset,
		startDate,
		endDate,
		groupBy: selectedGroupBy,
		routers: selectedRouters,
		routersLoaded,
		direction
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

	function moveCardBy(chartId: ChartCardId, offset: number) {
		const target = chartOrder[chartOrder.indexOf(chartId) + offset];
		if (!target) return;
		moveChartCard(chartId, target);
		persistChartOrder();
	}

	function resizeCard(chartId: ChartCardId, offset: number) {
		const frame = document.querySelector<HTMLElement>(`[data-chart-id="${chartId}"] .chart-frame`);
		if (!frame) return;
		const minimum = Number.parseFloat(getComputedStyle(frame).minHeight) || 160;
		chartHeights[chartId] = Math.max(
			minimum,
			Math.min(1200, frame.getBoundingClientRect().height + offset)
		);
		frame.style.removeProperty('height');
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

	onMount(() => {
		loadChartOrder();
		activateChart(chartOrder[0] ?? 'dashboard');
	});

	function handleDrillDown(payload: {
		groupBy: GroupByOption;
		startDate: string;
		endDate: string;
	}) {
		updateSearch(payload);
	}

	function handleMetricDrillDown(groupBy: GroupByOption, startDate: string, endDate: string) {
		handleDrillDown({ groupBy, startDate, endDate });
	}

	function handleMetricNavigateToFile(slug: string) {
		void navigateToNetflowFile(goto, slug, props.dataset, direction, ipVersion, measure);
	}

	function handleDataOptionsChange(payload: { options: DataOption[] }) {
		dataOptions = payload.options;
	}

	function handleIpMetricsChange(payload: { metrics: IpMetricKey[] }) {
		ipMetrics = payload.metrics;
	}

	function handleResetView() {
		updateSearch({ ...dateRangeSearch.defaults, endDate: today });
	}
</script>

<svelte:head>
	<title>{props.title ?? `ATLANTIS - ${props.dataset}`}</title>
	<meta name="description" content="NetFlow analysis and visualization tool" />
</svelte:head>

<div class="shell page flex flex-col gap-4">
	<div class="flex flex-wrap items-end justify-between gap-x-4 gap-y-1">
		<h1 class="text-xl font-semibold tracking-tight">{props.title ?? props.dataset}</h1>
		<p class="text-muted-foreground text-xs">
			Click a chart to drill down · drag across a chart to select a range
		</p>
	</div>
	<DashboardToolbar
		{startDate}
		{endDate}
		datasetStartDate={props.defaultStartDate}
		{today}
		groupBy={selectedGroupBy}
		routers={selectedRouters}
		{direction}
		showDirection={hasLocality}
		measure={maadComputed ? measure : null}
		{ipVersion}
		onDatesChange={(patch) => updateSearch(patch)}
		onGroupByChange={(groupBy) => updateSearch({ groupBy })}
		onRoutersChange={(routers) => (selectedRouters = routers)}
		onDirectionChange={(next) => updateSearch({ direction: next })}
		onMeasureChange={(next) => updateSearch({ measure: next })}
		onIpVersionChange={(next) => updateSearch({ ipVersion: next })}
		onReset={handleResetView}
	/>
	<KpiRow stats={netflowStats} />
	<div role="list" aria-label="Reorderable charts" class="chart-panels flex flex-col gap-3">
		{#each chartOrder as chartId, index (chartId)}
			<DashboardCardSlot
				title={CHART_CARD_DETAILS[chartId].title}
				first={index === 0}
				last={index === chartOrder.length - 1}
				resizable={activatedCharts[chartId] && chartId !== 'coverage'}
				onMove={(offset) => moveCardBy(chartId, offset)}
				onResize={(offset) => resizeCard(chartId, offset)}
				data-chart-id={chartId}
				data-chart-activated={activatedCharts[chartId]}
				class={`relative rounded-lg ${dropTargetChartId === chartId && draggedChartId && draggedChartId !== chartId ? 'ring-primary ring-offset-background ring-2 ring-offset-2' : ''}`}
				style={`${activatedCharts[chartId] ? '' : `min-height:${getCardMinimumHeight(chartId)}px;`}${chartHeights[chartId] ? `--chart-user-height:${chartHeights[chartId]}px` : ''}`}
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
					<Card.Root
						size="sm"
						class="h-full gap-0 py-0"
						style={`min-height:${getCardMinimumHeight(chartId)}px`}
						data-testid={`deferred-chart-${chartId}`}
					>
						<ChartCardHeader title={CHART_CARD_DETAILS[chartId].title} />
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
							<Button variant="outline" size="sm" onclick={() => activateChart(chartId)}>
								Load {CHART_CARD_DETAILS[chartId].title} chart
							</Button>
						</div>
					</Card.Root>
				{:else if chartId === 'dashboard'}
					<NetflowDashboard
						dataset={props.dataset}
						stats={netflowStats}
						groupBy={selectedGroupBy}
						{dataOptions}
						{direction}
						{ipVersion}
						{measure}
						onDrillDown={handleDrillDown}
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
						{direction}
						{ipVersion}
						onDrillDown={handleDrillDown}
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
						{direction}
						{ipVersion}
						onDrillDown={handleDrillDown}
						onMetricsChange={(payload) => {
							protocolMetrics = payload.metrics;
						}}
					/>
				{:else if chartId === 'dimensions'}
					<BreakdownChart
						kind="dimensions"
						dataset={props.dataset}
						{startDate}
						{endDate}
						granularity={ipGranularity}
						routers={selectedRouters}
						activeMetrics={dimensionMetrics}
						{ipVersion}
						{measure}
						unavailableCopy={maadUnavailableCopy}
						unavailableSideCopy={maadSideUnavailableCopy}
						{direction}
						onDrillDown={handleDrillDown}
						onMetricsChange={(payload) => {
							dimensionMetrics = payload.metrics;
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
						{ipVersion}
						{measure}
						unavailableCopy={spectrumUnavailableCopy}
						unavailableSideCopy={maadSideUnavailableCopy}
						availableRouters={availableSpectrumRouters}
						{direction}
						onDrillDown={handleDrillDown}
						onRouterChange={(payload) => {
							requestedSpectrumRouter = payload.router;
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
			</DashboardCardSlot>
		{/each}
	</div>
</div>
