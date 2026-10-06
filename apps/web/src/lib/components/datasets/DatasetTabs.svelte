<script lang="ts">
	import { resolve } from '$app/paths';
	let { datasetId, active }: { datasetId: string; active: 'dashboard' | 'alerts' } = $props();
</script>

<nav aria-label="Dataset views" class="dataset-tabs border-border flex gap-6 border-b">
	<a
		href={resolve('/datasets/[dataset]', { dataset: datasetId })}
		class="text-muted-foreground hover:text-foreground aria-[current=page]:border-primary aria-[current=page]:text-accent-foreground border-b-2 border-transparent px-1 py-2.5 text-sm"
		aria-current={active === 'dashboard' ? 'page' : undefined}>Dashboard</a
	>
	<a
		href={resolve('/datasets/[dataset]/alerts', { dataset: datasetId })}
		class="text-muted-foreground hover:text-foreground aria-[current=page]:border-primary aria-[current=page]:text-accent-foreground border-b-2 border-transparent px-1 py-2.5 text-sm"
		aria-current={active === 'alerts' ? 'page' : undefined}>Alerts</a
	>
</nav>
