<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { isGranularityAllowedForDateRange } from '$lib/components/charts/chart-utils';
	import SegmentedControl from '$lib/components/common/SegmentedControl.svelte';
	import DateRangeFilter from '$lib/components/filters/DateRangeFilter.svelte';
	import RouterFilter from '$lib/components/filters/RouterFilter.svelte';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types.ts';
	import { Button } from '$lib/components/ui/button';
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { FLOW_SCOPE_OPTIONS, type FlowScopeKey } from '$lib/types/types';

	interface GroupBySelectOption {
		value: GroupByOption;
		label: string;
	}

	const DEFAULT_GROUP_BY_OPTIONS: GroupBySelectOption[] = [
		{ value: 'date', label: 'Day' },
		{ value: 'hour', label: 'Hour' },
		{ value: '30min', label: '30 min' },
		{ value: '5min', label: '5 min' }
	];

	const props = $props<{
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		flowScope: FlowScopeKey;
		groupByOptions?: GroupBySelectOption[];
	}>();

	const dispatch = createEventDispatcher<{
		startDateChange: { startDate: string };
		endDateChange: { endDate: string };
		groupByChange: { groupBy: GroupByOption };
		routersChange: { routers: RouterConfig };
		scopeChange: { scope: FlowScopeKey };
		resetView: Record<string, never>;
	}>();

	function handleStartDateChange(date: string) {
		dispatch('startDateChange', { startDate: date });
	}

	function handleEndDateChange(date: string) {
		dispatch('endDateChange', { endDate: date });
	}

	function handleRoutersChange(nextRouters: RouterConfig) {
		dispatch('routersChange', { routers: nextRouters });
	}

	function handleScopeChange(event: Event) {
		const target = event.currentTarget as HTMLSelectElement;
		dispatch('scopeChange', { scope: target.value as FlowScopeKey });
	}

	function handleResetView() {
		dispatch('resetView', {});
	}

	const navigationTip = 'Click chart to drill down. Drag across chart to drill into a date range.';
	const groupByOptions = $derived(props.groupByOptions ?? DEFAULT_GROUP_BY_OPTIONS);

	function getGranularityDisabledReason(option: GroupBySelectOption): string | null {
		if (isGranularityAllowedForDateRange(option.value, props.startDate, props.endDate)) {
			return null;
		}

		return 'Date range too large for this granularity.';
	}

	const segmentedGroupByOptions = $derived(
		groupByOptions.map((option: GroupBySelectOption) => {
			const disabledReason = getGranularityDisabledReason(option);
			return {
				...option,
				disabled: disabledReason !== null,
				disabledReason: disabledReason ?? undefined
			};
		})
	);
</script>

<Card class="gap-3 rounded-lg border py-3 shadow-sm ring-0">
	<CardHeader class="px-3">
		<div class="flex items-center gap-2">
			<CardTitle class="text-sm font-semibold">Controls</CardTitle>
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props: triggerProps })}
						<Button
							{...triggerProps}
							variant="outline"
							size="icon-xs"
							class="text-muted-foreground hover:border-primary hover:text-primary size-5 rounded-full p-0 text-[11px] font-semibold"
							aria-label="Show navigation tip"
							title={navigationTip}
						>
							?
						</Button>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content side="bottom" sideOffset={4} class="w-64 leading-5">
					{navigationTip}
				</Tooltip.Content>
			</Tooltip.Root>
		</div>
	</CardHeader>

	<CardContent class="flex flex-wrap items-center gap-3 px-3">
		<DateRangeFilter
			startDate={props.startDate}
			endDate={props.endDate}
			onStartDateChange={handleStartDateChange}
			onEndDateChange={handleEndDateChange}
		/>

		<div class="bg-border hidden h-6 w-px sm:block" aria-hidden="true"></div>

		<SegmentedControl
			options={segmentedGroupByOptions}
			value={props.groupBy}
			onValueChange={(value) => dispatch('groupByChange', { groupBy: value })}
			class="min-w-[17rem] grid-cols-4"
			buttonClass="px-3 py-1 text-sm"
		/>

		<div class="bg-border hidden h-6 w-px sm:block" aria-hidden="true"></div>

		<RouterFilter routers={props.routers} onRouterChange={handleRoutersChange} />

		<label class="text-foreground flex items-center gap-2">
			<span class="text-sm font-medium">Scope:</span>
			<select
				value={props.flowScope}
				onchange={handleScopeChange}
				class="border-input bg-background text-foreground focus-visible:ring-ring rounded border px-2 py-1 text-sm focus-visible:ring-2 focus-visible:outline-none"
			>
				{#each FLOW_SCOPE_OPTIONS as option (option.key)}
					<option value={option.key}>{option.label}</option>
				{/each}
			</select>
		</label>

		<Button onclick={handleResetView} size="sm" class="h-7 px-4 sm:ml-auto">Reset View</Button>
	</CardContent>
</Card>
