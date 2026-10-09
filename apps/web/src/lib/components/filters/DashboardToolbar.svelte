<script lang="ts">
	import { RotateCcw } from '@lucide/svelte';
	import ToolbarSelect from '#lib/components/common/ToolbarSelect.svelte';
	import { isGranularityAllowedForDateRange } from '#lib/components/charts/chart-utils.ts';
	import type { GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
	import { Button } from '#lib/components/ui/button/index.ts';
	import {
		FLOW_DIRECTION_OPTIONS,
		type FlowDirection,
		type MaadIpVersion,
		type MaadMeasure
	} from '#lib/types/types.ts';
	import DateRangePicker from './DateRangePicker.svelte';
	import MaadOptions from './MaadOptions.svelte';
	import SourcesFilter from './SourcesFilter.svelte';

	let {
		startDate,
		endDate,
		datasetStartDate,
		today,
		groupBy,
		routers,
		direction,
		showDirection,
		measure,
		ipVersion,
		onDatesChange,
		onGroupByChange,
		onRoutersChange,
		onDirectionChange,
		onMeasureChange,
		onIpVersionChange,
		onReset
	}: {
		startDate: string;
		endDate: string;
		datasetStartDate: string;
		today: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		direction: FlowDirection;
		showDirection: boolean;
		measure: MaadMeasure | null;
		ipVersion: MaadIpVersion;
		onDatesChange: (patch: { startDate?: string; endDate?: string }) => void;
		onGroupByChange: (groupBy: GroupByOption) => void;
		onRoutersChange: (routers: RouterConfig) => void;
		onDirectionChange: (direction: FlowDirection) => void;
		onMeasureChange: (measure: MaadMeasure) => void;
		onIpVersionChange: (ipVersion: MaadIpVersion) => void;
		onReset: () => void;
	} = $props();

	const INTERVALS: Array<{ value: GroupByOption; label: string }> = [
		{ value: 'date', label: 'Day' },
		{ value: 'hour', label: 'Hour' },
		{ value: '30min', label: '30 min' },
		{ value: '10min', label: '10 min' },
		{ value: '5min', label: '5 min' }
	];

	const intervalOptions = $derived(
		INTERVALS.map((option) => {
			const allowed = isGranularityAllowedForDateRange(option.value, startDate, endDate);
			return {
				...option,
				disabled: !allowed,
				description: allowed ? undefined : 'Date range too large for this interval'
			};
		})
	);
	const directionOptions = FLOW_DIRECTION_OPTIONS.map((option) => ({
		value: option.value,
		label: option.label,
		description: option.value === 'all' ? undefined : option.description
	}));
</script>

<div class="dashboard-toolbar flex flex-wrap items-center gap-2" role="group" aria-label="Filters">
	<DateRangePicker {startDate} {endDate} {datasetStartDate} {today} onChange={onDatesChange} />
	<SourcesFilter {routers} {onRoutersChange} />
	<ToolbarSelect
		label="Interval"
		value={groupBy}
		options={intervalOptions}
		onValueChange={onGroupByChange}
	/>
	{#if showDirection}
		<ToolbarSelect
			label="Direction"
			value={direction}
			options={directionOptions}
			onValueChange={onDirectionChange}
		/>
	{/if}
	{#if measure}
		<MaadOptions {measure} {ipVersion} {onMeasureChange} {onIpVersionChange} />
	{/if}
	<Button
		variant="ghost"
		size="sm"
		class="toolbar-trigger text-muted-foreground ml-auto"
		aria-label="Reset filters"
		onclick={onReset}><RotateCcw class="size-3.5" />Reset</Button
	>
	{#if startDate > endDate}<p class="text-destructive w-full text-sm" role="alert">
			Start Date must be on or before End Date.
		</p>{/if}
</div>
