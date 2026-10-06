<script lang="ts">
	interface Props {
		startDate: string;
		endDate: string;
		onStartDateChange?: (date: string) => void;
		onEndDateChange?: (date: string) => void;
	}

	let { startDate, endDate, onStartDateChange, onEndDateChange }: Props = $props();

	function handleStartDateChange(event: Event) {
		const target = event.target as HTMLInputElement;
		onStartDateChange?.(target.value);
	}

	function handleEndDateChange(event: Event) {
		const target = event.target as HTMLInputElement;
		onEndDateChange?.(target.value);
	}
</script>

<div class="date-range-filter grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-end gap-2">
	<div class="min-w-0 flex-1">
		<label for="startDate" class="text-muted-foreground mb-1 block text-xs">Start Date</label>
		<input
			id="startDate"
			type="date"
			value={startDate}
			onchange={handleStartDateChange}
			class="border-input bg-background text-foreground focus-visible:ring-ring w-full min-w-0 rounded-md border px-2 py-1.5 text-sm focus-visible:ring-2 focus-visible:outline-none"
		/>
	</div>

	<span class="text-muted-foreground pb-1.5 text-sm">to</span>

	<div class="min-w-0 flex-1">
		<label for="endDate" class="text-muted-foreground mb-1 block text-xs">End Date</label>
		<input
			id="endDate"
			type="date"
			value={endDate}
			onchange={handleEndDateChange}
			class="border-input bg-background text-foreground focus-visible:ring-ring w-full min-w-0 rounded-md border px-2 py-1.5 text-sm focus-visible:ring-2 focus-visible:outline-none"
		/>
	</div>
</div>
