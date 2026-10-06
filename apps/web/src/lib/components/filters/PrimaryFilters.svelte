<script lang="ts">
	import { isGranularityAllowedForDateRange } from '#lib/components/charts/chart-utils.ts';
	import SegmentedControl from '#lib/components/common/SegmentedControl.svelte';
	import DirectionFilter from '#lib/components/filters/DirectionFilter.svelte';
	import MaadIpVersionFilter from '#lib/components/filters/MaadIpVersionFilter.svelte';
	import MaadMeasureFilter from '#lib/components/filters/MaadMeasureFilter.svelte';
	import RouterFilter from '#lib/components/filters/RouterFilter.svelte';
	import type { GroupByOption, RouterConfig } from '#lib/components/netflow/types.ts';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Tooltip from '#lib/components/ui/tooltip/index.ts';
	import {
		DEFAULT_MAAD_IP_VERSION,
		type FlowDirection,
		type MaadIpVersion,
		type MaadMeasure
	} from '#lib/types/types.ts';

	interface GroupBySelectOption {
		value: GroupByOption;
		label: string;
	}

	const DEFAULT_GROUP_BY_OPTIONS: GroupBySelectOption[] = [
		{ value: 'date', label: 'Day' },
		{ value: 'hour', label: 'Hour' },
		{ value: '30min', label: '30 min' },
		{ value: '10min', label: '10 min' },
		{ value: '5min', label: '5 min' }
	];

	const props = $props<{
		startDate: string;
		endDate: string;
		groupBy: GroupByOption;
		routers: RouterConfig;
		direction: FlowDirection;
		showDirection?: boolean;
		measure?: MaadMeasure;
		maadIpVersion?: MaadIpVersion;
		groupByOptions?: GroupBySelectOption[];
		onGroupByChange?: (payload: { groupBy: GroupByOption }) => void;
		onRoutersChange?: (payload: { routers: RouterConfig }) => void;
		onDirectionChange?: (payload: { direction: FlowDirection }) => void;
		onMeasureChange?: (payload: { measure: MaadMeasure }) => void;
		onMaadIpVersionChange?: (payload: { ipVersion: MaadIpVersion }) => void;
	}>();

	function handleRoutersChange(nextRouters: RouterConfig) {
		props.onRoutersChange?.({ routers: nextRouters });
	}

	function handleDirectionChange(payload: { direction: FlowDirection }) {
		props.onDirectionChange?.(payload);
	}

	const toolbarButtonClass = 'px-2 text-xs sm:px-3';
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

<div class="primary-filters">
	<div class="rail-section pt-0">
		<div class="mb-3 flex items-center justify-between gap-2">
			<h2 class="rail-heading">Controls</h2>
			<Tooltip.Root
				><Tooltip.Trigger
					>{#snippet child({ props: triggerProps })}<Button
							{...triggerProps}
							variant="ghost"
							size="icon-xs"
							aria-label="Show navigation tip"
							title={navigationTip}>?</Button
						>{/snippet}</Tooltip.Trigger
				><Tooltip.Content side="right" sideOffset={4} class="w-64 leading-5"
					>{navigationTip}</Tooltip.Content
				></Tooltip.Root
			>
		</div>
		<RouterFilter routers={props.routers} onRouterChange={handleRoutersChange} />
	</div>
	<div class="rail-section">
		<h2 class="rail-heading">Granularity</h2>
		<SegmentedControl
			options={segmentedGroupByOptions}
			value={props.groupBy}
			onValueChange={(value) => props.onGroupByChange?.({ groupBy: value })}
			ariaLabel="Granularity"
			class="rail-segments grid-cols-3"
			buttonClass={toolbarButtonClass}
		/>
	</div>
	{#if props.showDirection ?? true}<div class="rail-section">
			<h2 class="rail-heading">Direction</h2>
			<DirectionFilter
				direction={props.direction}
				onDirectionChange={handleDirectionChange}
				buttonClass={toolbarButtonClass}
			/>
		</div>{/if}
	{#if props.measure}<div class="rail-section" role="group" aria-label="MAAD options">
			<h2 class="rail-heading">MAAD</h2>
			<p class="text-muted-foreground mb-2 text-xs">Measure</p>
			<MaadMeasureFilter
				measure={props.measure}
				onMeasureChange={props.onMeasureChange}
				buttonClass={toolbarButtonClass}
			/>
			<p class="text-muted-foreground mt-3 mb-2 text-xs">Address family</p>
			<MaadIpVersionFilter
				ipVersion={props.maadIpVersion ?? DEFAULT_MAAD_IP_VERSION}
				onIpVersionChange={props.onMaadIpVersionChange}
				buttonClass={toolbarButtonClass}
			/>
		</div>{/if}
	<p class="text-muted-foreground mt-4 text-xs leading-5">
		Click a chart to drill down. Drag across a chart to select a date range.
	</p>
</div>
