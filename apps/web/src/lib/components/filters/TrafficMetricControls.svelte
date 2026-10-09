<script lang="ts">
	import SegmentedToggle from '#lib/components/common/SegmentedToggle.svelte';
	import ToolbarPopover from '#lib/components/common/ToolbarPopover.svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import { Checkbox } from '#lib/components/ui/checkbox/index.ts';
	import type { DataOption } from '#lib/components/netflow/types.ts';

	type MetricFamily = 'flows' | 'packets' | 'bytes';
	type ProtocolFamily = 'tcp' | 'udp' | 'icmp' | 'other';

	let {
		dataOptions,
		onDataOptionsChange
	}: {
		dataOptions: DataOption[];
		onDataOptionsChange: (dataOptions: DataOption[]) => void;
	} = $props();

	const METRIC_ROWS: Array<{ value: MetricFamily; label: string }> = [
		{ value: 'flows', label: 'Flows' },
		{ value: 'packets', label: 'Packets' },
		{ value: 'bytes', label: 'Bytes' }
	];
	const PROTOCOL_COLUMNS: Array<{ value: ProtocolFamily; label: string }> = [
		{ value: 'tcp', label: 'TCP' },
		{ value: 'udp', label: 'UDP' },
		{ value: 'icmp', label: 'ICMP' },
		{ value: 'other', label: 'Other' }
	];

	function metricFamily(option: DataOption): MetricFamily | null {
		const normalized = option.label.toLowerCase();
		if (normalized.includes('flow')) return 'flows';
		if (normalized.includes('packet')) return 'packets';
		if (normalized.includes('byte')) return 'bytes';
		return null;
	}

	function protocolFamily(option: DataOption): ProtocolFamily | null {
		const normalized = option.label.toLowerCase();
		for (const protocol of PROTOCOL_COLUMNS) {
			if (normalized.includes(protocol.value)) return protocol.value;
		}
		return null;
	}

	const selectedFamily = $derived(
		METRIC_ROWS.find((family) =>
			dataOptions.every((option) => option.checked === (metricFamily(option) === family.value))
		)?.value ?? null
	);
	const checkedCount = $derived(dataOptions.filter((option) => option.checked).length);
	const matrix = $derived(
		METRIC_ROWS.map((metric) => ({
			...metric,
			cells: PROTOCOL_COLUMNS.map((protocol) => ({
				...protocol,
				option: dataOptions.find(
					(option) =>
						metricFamily(option) === metric.value && protocolFamily(option) === protocol.value
				)
			}))
		}))
	);

	function selectFamily(family: MetricFamily) {
		onDataOptionsChange(
			dataOptions.map((option) => ({ ...option, checked: metricFamily(option) === family }))
		);
	}

	function setAll(checked: boolean) {
		onDataOptionsChange(dataOptions.map((option) => ({ ...option, checked })));
	}

	function toggle(index: number) {
		onDataOptionsChange(
			dataOptions.map((option) =>
				option.index === index ? { ...option, checked: !option.checked } : option
			)
		);
	}
</script>

<SegmentedToggle
	options={METRIC_ROWS}
	value={selectedFamily}
	onValueChange={selectFamily}
	ariaLabel="Traffic metric family"
/>
<ToolbarPopover
	label="Metrics"
	value={`${checkedCount}/${dataOptions.length}`}
	dialogLabel="NetFlow metrics"
	variant="ghost"
	align="end"
>
	<div class="flex items-center justify-between gap-2 border-b px-3 py-2">
		<p class="text-xs font-medium">NetFlow metrics</p>
		<div class="flex gap-0.5">
			<Button variant="ghost" size="xs" onclick={() => setAll(true)}>All</Button>
			<Button variant="ghost" size="xs" onclick={() => setAll(false)}>None</Button>
		</div>
	</div>
	<div class="p-2" role="group" aria-label="NetFlow metrics">
		<div
			class="text-muted-foreground grid grid-cols-[4.5rem_repeat(4,2.75rem)] items-center text-center text-xs"
		>
			<span></span>
			{#each PROTOCOL_COLUMNS as protocol (protocol.value)}<span class="py-1">{protocol.label}</span
				>{/each}
		</div>
		{#each matrix as metric (metric.value)}
			<div class="grid grid-cols-[4.5rem_repeat(4,2.75rem)] items-center">
				<span class="px-1 text-sm font-medium">{metric.label}</span>
				{#each metric.cells as cell (cell.value)}
					<label
						class="hover:bg-muted flex h-9 cursor-pointer items-center justify-center rounded-sm"
					>
						{#if cell.option}
							{@const option = cell.option}
							<Checkbox checked={option.checked} onCheckedChange={() => toggle(option.index)} />
							<span class="sr-only">{option.label}</span>
						{/if}
					</label>
				{/each}
			</div>
		{/each}
	</div>
</ToolbarPopover>
