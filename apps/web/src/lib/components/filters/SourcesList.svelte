<script lang="ts">
	import { Check } from '@lucide/svelte';
	import * as Command from '#lib/components/ui/command/index.ts';
	import { focusSearch } from '#lib/components/common/focus-search.ts';
	import type { RouterConfig } from '#lib/components/netflow/types.ts';

	let {
		routers,
		onRoutersChange
	}: { routers: RouterConfig; onRoutersChange: (routers: RouterConfig) => void } = $props();

	const names = $derived(Object.keys(routers));
</script>

<div class="contents" {@attach focusSearch}>
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
</div>
