<script lang="ts">
	import { Check } from '@lucide/svelte';
	import ToolbarPopover from '#lib/components/common/ToolbarPopover.svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Command from '#lib/components/ui/command/index.ts';
	import type { RouterConfig } from '#lib/components/netflow/types.ts';

	let {
		routers,
		onRoutersChange
	}: { routers: RouterConfig; onRoutersChange: (routers: RouterConfig) => void } = $props();

	const names = $derived(Object.keys(routers));
	const selectedCount = $derived(names.filter((name) => routers[name]).length);

	function setAll(enabled: boolean) {
		onRoutersChange(Object.fromEntries(names.map((name) => [name, enabled])));
	}
</script>

<ToolbarPopover
	label="Sources"
	value={`${selectedCount}/${names.length}`}
	ariaLabel={`Sources: ${selectedCount} of ${names.length} selected`}
	dialogLabel="Sources"
	contentClass="w-64"
>
	<Command.Root>
		<Command.Input placeholder="Search sources…" aria-label="Search sources" />
		<Command.List class="max-h-64" aria-label="Sources">
			<Command.Empty>No sources found.</Command.Empty>
			{#each names as name (name)}
				<Command.Item
					value={name}
					aria-checked={routers[name]}
					onSelect={() => onRoutersChange({ ...routers, [name]: !routers[name] })}
					class="gap-2.5"
				>
					<span
						class={[
							'flex size-4 shrink-0 items-center justify-center rounded-[4px] border shadow-xs',
							routers[name]
								? 'border-primary bg-primary text-primary-foreground'
								: 'border-input bg-background'
						]}
						aria-hidden="true"
					>
						{#if routers[name]}<Check class="size-3 text-current" />{/if}
					</span>
					<span class="truncate">{name}</span>
				</Command.Item>
			{/each}
		</Command.List>
	</Command.Root>
	<div class="flex items-center justify-between gap-2 border-t px-2 py-1.5">
		<span class="text-muted-foreground text-xs">{selectedCount} selected</span>
		<div class="flex gap-0.5">
			<Button variant="ghost" size="xs" onclick={() => setAll(true)}>Select all</Button>
			<Button variant="ghost" size="xs" onclick={() => setAll(false)}>Clear</Button>
		</div>
	</div>
</ToolbarPopover>
