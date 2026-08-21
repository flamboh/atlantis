<script lang="ts">
	import { Checkbox } from '$lib/components/ui/checkbox';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import type { RouterConfig } from '$lib/components/netflow/types.ts';

	interface Props {
		routers: RouterConfig;
		onRouterChange?: (routers: RouterConfig) => void;
	}

	let { routers, onRouterChange }: Props = $props();
	const routerNames = $derived(Object.keys(routers));

	function handleRouterToggle(routerName: string) {
		const newRouters = {
			...routers,
			[routerName]: !routers[routerName]
		};
		onRouterChange?.(newRouters);
	}
</script>

<div class="router-filter flex flex-wrap items-center gap-3">
	<span class="text-foreground text-sm font-medium">Sources:</span>

	<div class="flex min-h-0 flex-wrap items-center gap-3">
		{#if routerNames.length === 0}
			{#each Array(4) as _, index (index)}
				<Skeleton class="inline-block h-4 w-24" aria-hidden="true" />
			{/each}
		{:else}
			{#each routerNames as routerName (routerName)}
				<label class="hover:bg-muted flex cursor-pointer items-center gap-2 rounded-md px-1 py-1">
					<Checkbox
						checked={routers[routerName]}
						onCheckedChange={() => handleRouterToggle(routerName)}
					/>
					<span class="text-foreground text-sm">{routerName}</span>
				</label>
			{/each}
		{/if}
	</div>
</div>
