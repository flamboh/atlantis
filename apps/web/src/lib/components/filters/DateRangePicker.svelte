<script lang="ts">
	import { parseDate, type DateValue } from '@internationalized/date';
	import { CalendarDays } from '@lucide/svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import ToolbarPopover from '#lib/components/common/ToolbarPopover.svelte';
	import { Input } from '#lib/components/ui/input/index.ts';
	import { Label } from '#lib/components/ui/label/index.ts';
	import { RangeCalendar } from '#lib/components/ui/range-calendar/index.ts';
	import { dateRangePresets, formatDateRange, isIsoDate } from './date-range.ts';

	type Range = { start: DateValue | undefined; end: DateValue | undefined };

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
	let draft = $state<Range>({ start: undefined, end: undefined });
	let placeholder = $state<DateValue | undefined>(undefined);
	const label = $derived(formatDateRange(startDate, endDate));
	const presets = $derived(dateRangePresets(endDate, datasetStartDate, today));

	function syncDraft() {
		const start = isIsoDate(startDate) ? parseDate(startDate) : undefined;
		const end = isIsoDate(endDate) ? parseDate(endDate) : undefined;
		const [low, high] = start && end && start.compare(end) > 0 ? [end, start] : [start, end];
		draft = { start: low, end: high };
		const focus = high ?? low ?? parseDate(today);
		placeholder = wide.current ? focus.subtract({ months: 1 }) : focus;
	}

	function apply(next: { startDate: string; endDate: string }) {
		onChange(next);
		open = false;
	}

	function commitField(side: 'startDate' | 'endDate', value: string) {
		if (isIsoDate(value)) onChange({ [side]: value });
	}
</script>

<ToolbarPopover
	bind:open
	onOpenChange={(next) => {
		if (next) syncDraft();
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
			<RangeCalendar
				bind:value={draft}
				bind:placeholder
				numberOfMonths={wide.current ? 2 : 1}
				weekStartsOn={0}
				onValueChange={(next) => {
					if (next.start && next.end)
						apply({ startDate: next.start.toString(), endDate: next.end.toString() });
				}}
			/>
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
						aria-invalid={!isIsoDate(startDate) || undefined}
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
						aria-invalid={!isIsoDate(endDate) || undefined}
						onchange={(event) => commitField('endDate', event.currentTarget.value.trim())}
					/>
				</div>
			</div>
		</div>
	</div>
</ToolbarPopover>
