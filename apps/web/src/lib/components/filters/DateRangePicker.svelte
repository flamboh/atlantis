<script lang="ts">
	import { CalendarDays } from '@lucide/svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import ToolbarPopover from '#lib/components/common/ToolbarPopover.svelte';
	import { Input } from '#lib/components/ui/input/index.ts';
	import { Label } from '#lib/components/ui/label/index.ts';
	import { Skeleton } from '#lib/components/ui/skeleton/index.ts';
	import { dateRangePresets, formatDateRange, isIsoDate } from './date-range.ts';

	let {
		startDate,
		endDate,
		datasetStartDate,
		today,
		onChange
	}: {
		startDate: string;
		endDate: string;
		datasetStartDate: string;
		today: string;
		onChange: (patch: { startDate?: string; endDate?: string }) => void;
	} = $props();

	const id = $props.id();
	const wide = new MediaQuery('(min-width: 640px)', true);
	let open = $state(false);
	let invalid = $state({ startDate: false, endDate: false });
	let calendar = $state<Promise<typeof import('./DateRangeCalendar.svelte')> | null>(null);
	const label = $derived(formatDateRange(startDate, endDate));
	const presets = $derived(dateRangePresets(endDate, datasetStartDate, today));

	function apply(next: { startDate: string; endDate: string }) {
		onChange(next);
		open = false;
	}

	function commitField(side: 'startDate' | 'endDate', value: string) {
		invalid[side] = !isIsoDate(value);
		if (!invalid[side]) onChange({ [side]: value });
	}
</script>

<ToolbarPopover
	bind:open
	onOpenChange={(next) => {
		if (!next) return;
		invalid = { startDate: false, endDate: false };
		calendar ??= import('./DateRangeCalendar.svelte');
	}}
	{label}
	ariaLabel={`Date range: ${label}`}
	dialogLabel="Date range"
>
	{#snippet icon()}<CalendarDays class="text-muted-foreground size-4" />{/snippet}
	<div class="flex flex-col sm:flex-row">
		<div
			class="flex gap-1 overflow-x-auto border-b p-2 sm:w-36 sm:flex-col sm:border-r sm:border-b-0"
			role="group"
			aria-label="Date range presets"
		>
			{#each presets as preset (preset.id)}
				<button
					type="button"
					class="hover:bg-muted aria-pressed:bg-muted aria-pressed:text-foreground text-muted-foreground rounded-sm px-2 py-1.5 text-left text-sm whitespace-nowrap"
					aria-pressed={preset.startDate === startDate && preset.endDate === endDate}
					onclick={() => apply(preset)}>{preset.label}</button
				>
			{/each}
			<p class="text-muted-foreground mt-auto hidden px-2 pt-2 text-xs leading-4 sm:block">
				Windows end on the selected end date.
			</p>
		</div>
		<div class="flex flex-col">
			{#if calendar}
				{#await calendar}
					<Skeleton class="m-3 h-72 w-64 sm:w-[34rem]" />
				{:then { default: DateRangeCalendar }}
					<DateRangeCalendar
						{startDate}
						{endDate}
						{today}
						months={wide.current ? 2 : 1}
						onApply={apply}
					/>
				{/await}
			{/if}
			<div class="grid grid-cols-2 gap-3 border-t p-3">
				<div class="grid gap-1.5">
					<Label for={`${id}-start`} class="text-muted-foreground text-xs font-normal"
						>Start Date</Label
					>
					<Input
						id={`${id}-start`}
						value={startDate}
						placeholder="YYYY-MM-DD"
						inputmode="numeric"
						autocomplete="off"
						class="h-8 font-mono text-xs"
						aria-invalid={invalid.startDate || undefined}
						onchange={(event) => commitField('startDate', event.currentTarget.value.trim())}
					/>
				</div>
				<div class="grid gap-1.5">
					<Label for={`${id}-end`} class="text-muted-foreground text-xs font-normal">End Date</Label
					>
					<Input
						id={`${id}-end`}
						value={endDate}
						placeholder="YYYY-MM-DD"
						inputmode="numeric"
						autocomplete="off"
						class="h-8 font-mono text-xs"
						aria-invalid={invalid.endDate || undefined}
						onchange={(event) => commitField('endDate', event.currentTarget.value.trim())}
					/>
				</div>
			</div>
		</div>
	</div>
</ToolbarPopover>
