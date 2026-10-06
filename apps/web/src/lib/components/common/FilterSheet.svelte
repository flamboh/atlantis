<script lang="ts">
	import { Dialog } from 'bits-ui';
	import { X } from '@lucide/svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import type { Snippet } from 'svelte';

	let {
		open = $bindable(false),
		title,
		children,
		onClose
	}: { open?: boolean; title: string; children: Snippet; onClose?: () => void } = $props();
</script>

<Dialog.Root bind:open>
	<Dialog.Portal>
		<Dialog.Overlay class="fixed inset-0 z-50 bg-black/40" />
		<Dialog.Content
			onCloseAutoFocus={(event) => {
				if (onClose) {
					event.preventDefault();
					onClose();
				}
			}}
			class="filter-sheet bg-card text-card-foreground fixed inset-y-0 left-0 z-50 flex w-[min(22rem,calc(100vw-1rem))] flex-col border-r shadow-xl"
		>
			<div class="border-border flex shrink-0 items-center justify-between border-b px-4 py-3">
				<Dialog.Title class="text-sm font-semibold">{title}</Dialog.Title>
				<Dialog.Close>
					{#snippet child({ props })}
						<Button
							{...props}
							variant="ghost"
							size="icon-sm"
							aria-label={`Close ${title.toLowerCase()}`}><X size={18} /></Button
						>
					{/snippet}
				</Dialog.Close>
			</div>
			<Dialog.Description class="sr-only"
				>These options apply to the current view. Close this panel to return to the results.</Dialog.Description
			>
			<div class="min-h-0 flex-1 overflow-y-auto p-4">{@render children()}</div>
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>
