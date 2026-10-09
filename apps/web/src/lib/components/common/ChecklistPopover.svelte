<script lang="ts" generics="K extends string">
	import { Button } from '#lib/components/ui/button/index.ts';
	import { Checkbox } from '#lib/components/ui/checkbox/index.ts';
	import ToolbarPopover from './ToolbarPopover.svelte';

	let {
		label = 'Metrics',
		dialogLabel,
		items,
		onToggle,
		onSetAll
	}: {
		label?: string;
		dialogLabel: string;
		items: readonly { key: K; label: string; checked: boolean }[];
		onToggle: (key: K) => void;
		onSetAll?: (checked: boolean) => void;
	} = $props();

	const checkedCount = $derived(items.filter((item) => item.checked).length);
</script>

<ToolbarPopover
	{label}
	value={`${checkedCount}/${items.length}`}
	{dialogLabel}
	variant="ghost"
	align="end"
	contentClass="w-60"
>
	<div class="flex items-center justify-between gap-2 border-b px-3 py-2">
		<p class="text-xs font-medium">{dialogLabel}</p>
		{#if onSetAll}
			<div class="flex gap-0.5">
				<Button variant="ghost" size="xs" onclick={() => onSetAll(true)}>All</Button>
				<Button variant="ghost" size="xs" onclick={() => onSetAll(false)}>None</Button>
			</div>
		{/if}
	</div>
	<div class="flex flex-col p-1" role="group" aria-label={dialogLabel}>
		{#each items as item (item.key)}
			<label
				class="hover:bg-muted flex cursor-pointer items-center gap-2.5 rounded-sm px-2 py-1.5 text-sm"
			>
				<Checkbox checked={item.checked} onCheckedChange={() => onToggle(item.key)} />
				<span>{item.label}</span>
			</label>
		{/each}
	</div>
</ToolbarPopover>
