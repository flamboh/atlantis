<script lang="ts">
	import { tick } from 'svelte';
	import { ArrowDown, ArrowUp, Ellipsis, Minus, Plus } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as DropdownMenu from '#lib/components/ui/dropdown-menu/index.ts';
	import type { CardFrame } from './card-frame.ts';

	let { frame }: { frame: CardFrame } = $props();
	let trigger = $state<HTMLButtonElement | null>(null);
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				bind:ref={trigger}
				variant="ghost"
				size="icon-sm"
				class="text-muted-foreground hover:text-foreground"
				aria-label={`${frame.title} card options`}
				data-card-menu-trigger
			>
				<Ellipsis class="size-4" />
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content
		align="end"
		class="w-44"
		onCloseAutoFocus={(event) => {
			event.preventDefault();
			// Moves re-insert the card, so wait for the list to settle; the card slot also refocuses
			// after moves in case this menu unmounted with a placeholder header.
			void tick().then(() => trigger?.focus());
		}}
	>
		<DropdownMenu.Group>
			<DropdownMenu.Label class="text-muted-foreground truncate text-xs font-normal"
				>{frame.title}</DropdownMenu.Label
			>
			<DropdownMenu.Item disabled={frame.first} onSelect={() => frame.move(-1)}
				><ArrowUp />Move up</DropdownMenu.Item
			>
			<DropdownMenu.Item disabled={frame.last} onSelect={() => frame.move(1)}
				><ArrowDown />Move down</DropdownMenu.Item
			>
		</DropdownMenu.Group>
		{#if frame.resizable}
			<DropdownMenu.Separator />
			<DropdownMenu.Group>
				<DropdownMenu.Item onSelect={() => frame.resize(80)}><Plus />Taller</DropdownMenu.Item>
				<DropdownMenu.Item onSelect={() => frame.resize(-80)}><Minus />Shorter</DropdownMenu.Item>
			</DropdownMenu.Group>
		{/if}
	</DropdownMenu.Content>
</DropdownMenu.Root>
