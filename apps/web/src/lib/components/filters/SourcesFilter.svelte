<script lang="ts">
	import ToolbarPopover from '#lib/components/common/ToolbarPopover.svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import type { RouterConfig } from '#lib/components/netflow/types.ts';

	let {
		routers,
		onRoutersChange
	}: { routers: RouterConfig; onRoutersChange: (routers: RouterConfig) => void } = $props();

	let list = $state<Promise<typeof import('./SourcesList.svelte')> | null>(null);
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
	focusContentOnOpen
	onOpenChange={(next) => {
		if (next) list ??= import('./SourcesList.svelte');
	}}
>
	{#if list}
		{#await list}
			<div class="text-muted-foreground px-3 py-6 text-center text-sm" role="status">
				Loading sources…
			</div>
		{:then { default: SourcesList }}
			<SourcesList {routers} {onRoutersChange} />
		{/await}
	{/if}
	<div class="flex items-center justify-between gap-2 border-t px-2 py-1.5">
		<span class="text-muted-foreground text-xs">{selectedCount} selected</span>
		<div class="flex gap-0.5">
			<Button variant="ghost" size="xs" onclick={() => setAll(true)}>Select all</Button>
			<Button variant="ghost" size="xs" onclick={() => setAll(false)}>Clear</Button>
		</div>
	</div>
</ToolbarPopover>
