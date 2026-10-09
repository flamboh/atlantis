<script lang="ts">
	import type { Snippet } from 'svelte';

	let { title, description, actions }: { title: string; description?: Snippet; actions?: Snippet } =
		$props();
</script>

<div class="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
	<div class="min-w-0">
		<h1 class="text-xl font-semibold tracking-tight break-words">{title}</h1>
		{#if description}<div class="text-muted-foreground mt-1 text-sm">
				{@render description()}
			</div>{/if}
	</div>
	{#if actions}<div class="flex flex-wrap items-center gap-2">{@render actions()}</div>{/if}
</div>
