<script lang="ts">
	import { sectionHref, sectionsFor, type AppSection } from './app-context.ts';

	let { datasetId, active }: { datasetId: string | null; active: AppSection } = $props();
</script>

<nav aria-label="Main navigation" class="app-nav border-border bg-card border-b">
	<div class="shell flex gap-1 overflow-x-auto">
		{#each sectionsFor(datasetId) as item (item.section)}
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- sectionHref resolves every path. -->
			<a
				href={sectionHref(item.section, datasetId)}
				aria-current={active === item.section ? 'page' : undefined}
				class="text-muted-foreground hover:text-foreground aria-[current=page]:border-foreground aria-[current=page]:text-foreground -mb-px border-b-2 border-transparent px-2.5 py-2.5 text-sm whitespace-nowrap transition-colors aria-[current=page]:font-medium"
				>{item.label}</a
			>
		{/each}
	</div>
</nav>
