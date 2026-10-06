<script lang="ts">
	let { label }: { label: string } = $props();
</script>

<div class="relative flex h-full w-full items-center justify-center overflow-hidden" role="status">
	<div
		class="pointer-events-none absolute inset-6 flex flex-col justify-between opacity-60"
		aria-hidden="true"
	>
		{#each [0, 1, 2, 3] as line (line)}<div class="border-border w-full border-t"></div>{/each}
		<div class="bg-muted absolute bottom-0 h-1/3 w-full animate-pulse rounded-sm"></div>
	</div>
	<span class="bg-card text-muted-foreground relative rounded px-3 py-2 text-sm">{label}</span>
</div>
