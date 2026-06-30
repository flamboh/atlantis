<script lang="ts">
	import { createEventDispatcher } from 'svelte';
	import { isGranularityAllowedForDateRange } from '$lib/components/charts/chart-utils';
	import DateRangeFilter from '$lib/components/filters/DateRangeFilter.svelte';
	import RouterFilter from '$lib/components/filters/RouterFilter.svelte';
	import type { GroupByOption, RouterConfig } from '$lib/components/netflow/types.ts';
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
	const selectedGroupByIndex = $derived(
		Math.max(
			0,
			groupByOptions.findIndex((option: GroupBySelectOption) => option.value === props.groupBy)
		)
	);

	function getGranularityDisabledReason(option: GroupBySelectOption): string | null {
		if (isGranularityAllowedForDateRange(option.value, props.startDate, props.endDate)) {
			return null;
		}

		return 'Date range too large for this granularity.';
	}
</script>

<div
	class="dark:border-dark-border dark:bg-dark-surface space-y-3 rounded-lg border bg-white p-3 shadow-sm dark:shadow-none"
>
	<div class="flex items-center gap-2">
		<h2 class="text-sm font-semibold text-gray-900 dark:text-gray-100">Controls</h2>
		<div class="group relative">
			<button
				type="button"
				class="dark:border-dark-border inline-flex h-5 w-5 items-center justify-center rounded-full border border-gray-300 text-[11px] font-semibold text-gray-500 hover:border-blue-500 hover:text-blue-600 focus:ring-2 focus:ring-blue-500 focus:outline-none dark:text-gray-400"
				aria-label="Show navigation tip"
				title={navigationTip}
			>
				?
			</button>
			<div
				class="dark:border-dark-border dark:bg-dark-subtle pointer-events-none absolute left-0 z-10 mt-2 hidden w-64 rounded-md border border-gray-200 bg-white p-2 text-xs leading-5 text-gray-600 shadow-lg group-focus-within:block group-hover:block dark:text-gray-400"
				role="tooltip"
			>
				{navigationTip}
			</div>
		</div>
	</div>

	<div class="flex flex-wrap items-center gap-3">
		<DateRangeFilter
			startDate={props.startDate}
			endDate={props.endDate}
			onStartDateChange={handleStartDateChange}
			onEndDateChange={handleEndDateChange}
		/>

		<div class="dark:bg-dark-border hidden h-6 w-px bg-gray-200 sm:block" aria-hidden="true"></div>

		<div
			class="dark:border-dark-border dark:bg-dark-subtle relative inline-grid min-w-[17rem] grid-cols-4 rounded-md border border-gray-200 bg-gray-50 p-1"
		>
			<div
				class="pointer-events-none absolute top-1 bottom-1 rounded bg-blue-600 shadow-sm transition-transform duration-200 ease-out will-change-transform motion-reduce:transition-none"
				style={`left: 0.25rem; width: calc((100% - 0.5rem) / 4); transform: translateX(${selectedGroupByIndex * 100}%);`}
				aria-hidden="true"
			></div>
			{#each groupByOptions as option (option.value)}
				{@const disabledReason = getGranularityDisabledReason(option)}
				<div class="group relative flex">
					<button
						type="button"
						onclick={() =>
							!disabledReason &&
							props.groupBy !== option.value &&
							dispatch('groupByChange', { groupBy: option.value })}
						class={`relative z-10 flex w-full items-center justify-center rounded px-3 py-1 text-center text-sm transition-colors focus:ring-2 focus:ring-blue-500 focus:outline-none ${
							props.groupBy === option.value
								? 'text-white'
								: disabledReason
									? 'text-gray-400'
									: 'text-gray-600 hover:text-gray-900 dark:text-gray-300 dark:hover:text-gray-100'
						}`}
						aria-pressed={props.groupBy === option.value}
						aria-disabled={disabledReason ? 'true' : 'false'}
					>
						{option.label}
					</button>
					{#if disabledReason}
						<div
							class="dark:border-dark-border dark:bg-dark-subtle pointer-events-none absolute top-full left-1/2 z-20 mt-1 hidden w-44 -translate-x-1/2 rounded-md border border-gray-200 bg-white p-2 text-xs leading-4 text-gray-600 shadow-lg group-focus-within:block group-hover:block dark:text-gray-400"
							role="tooltip"
						>
							{disabledReason}
						</div>
					{/if}
				</div>
			{/each}
		</div>

		<div class="dark:bg-dark-border hidden h-6 w-px bg-gray-200 sm:block" aria-hidden="true"></div>

		<RouterFilter routers={props.routers} onRouterChange={handleRoutersChange} />

		<label class="flex items-center gap-2 text-gray-700 dark:text-gray-200">
			<span class="text-sm font-medium text-gray-700 dark:text-gray-300">Scope:</span>
			<select
				value={props.flowScope}
				onchange={handleScopeChange}
				class="dark:border-dark-border dark:bg-dark-bg rounded border border-gray-300 bg-white px-2 py-1 text-sm text-gray-900 dark:text-gray-100"
			>
				{#each FLOW_SCOPE_OPTIONS as option (option.key)}
					<option value={option.key}>{option.label}</option>
				{/each}
			</select>
		</label>

		<button
			type="button"
			onclick={handleResetView}
			class="rounded-md bg-blue-600 px-4 py-1 text-sm text-white hover:bg-blue-700 focus:ring-2 focus:ring-blue-500 focus:outline-none sm:ml-auto"
		>
			Reset View
		</button>
	</div>
</div>
