<script lang="ts">
	import { parseDate, type DateValue } from '@internationalized/date';
	import { untrack } from 'svelte';
	import { RangeCalendar } from '#lib/components/ui/range-calendar/index.ts';
	import { isIsoDate } from './date-range.ts';

	let {
		startDate,
		endDate,
		today,
		months,
		onApply
	}: {
		startDate: string;
		endDate: string;
		today: string;
		months: number;
		onApply: (range: { startDate: string; endDate: string }) => void;
	} = $props();

	// The popover mounts this panel on open, so the draft starts from the committed range each time.
	const initial = untrack(() => {
		const start = isIsoDate(startDate) ? parseDate(startDate) : undefined;
		const end = isIsoDate(endDate) ? parseDate(endDate) : undefined;
		const [low, high] = start && end && start.compare(end) > 0 ? [end, start] : [start, end];
		const focus = high ?? low ?? parseDate(today);
		return {
			draft: { start: low, end: high },
			placeholder: months > 1 ? focus.subtract({ months: months - 1 }) : focus
		};
	});
	let draft = $state<{ start: DateValue | undefined; end: DateValue | undefined }>(initial.draft);
	let placeholder = $state<DateValue>(initial.placeholder);
</script>

<RangeCalendar
	bind:value={draft}
	bind:placeholder
	numberOfMonths={months}
	weekStartsOn={0}
	class="[&_[data-outside-month]]:invisible"
	onValueChange={(next) => {
		if (next.start && next.end)
			onApply({ startDate: next.start.toString(), endDate: next.end.toString() });
	}}
/>
