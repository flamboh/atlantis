<script lang="ts">
	import { Popover } from 'bits-ui';
	import { ChevronDown, X } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import type { Snippet } from 'svelte';

	let { label, summary, children }: { label: string; summary: string; children: Snippet } =
		$props();
</script>

<Popover.Root>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="outline" size="sm" aria-label={label}
				>{summary}<ChevronDown size={13} /></Button
			>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content
		side="bottom"
		align="start"
		sideOffset={6}
		class="control-popover bg-popover text-popover-foreground z-40 w-[min(24rem,calc(100vw-2rem))] rounded-md border p-3 shadow-lg"
		aria-label={label}
	>
		<div class="mb-3 flex items-center justify-between gap-2">
			<p class="text-sm font-semibold">{label}</p>
			<Popover.Close>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="ghost"
						size="icon-xs"
						aria-label={`Close ${label.toLowerCase()}`}><X size={14} /></Button
					>
				{/snippet}
			</Popover.Close>
		</div>
		{@render children()}
	</Popover.Content>
</Popover.Root>
