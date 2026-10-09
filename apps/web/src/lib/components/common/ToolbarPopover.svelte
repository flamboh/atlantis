<script lang="ts">
	import { ChevronDown } from '@lucide/svelte';
	import type { Snippet } from 'svelte';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Popover from '#lib/components/ui/popover/index.ts';
	import { cn } from '#lib/utils.ts';

	let {
		label,
		value,
		ariaLabel,
		dialogLabel = label,
		variant = 'outline',
		align = 'start',
		open = $bindable(false),
		onOpenChange,
		focusContentOnOpen = false,
		icon,
		class: className,
		contentClass,
		children
	}: {
		label: string;
		value?: string;
		ariaLabel?: string;
		dialogLabel?: string;
		variant?: 'outline' | 'ghost';
		align?: 'start' | 'center' | 'end';
		open?: boolean;
		onOpenChange?: (open: boolean) => void;
		focusContentOnOpen?: boolean;
		icon?: Snippet;
		class?: string;
		contentClass?: string;
		children: Snippet;
	} = $props();
	let content = $state<HTMLElement | null>(null);
</script>

<Popover.Root bind:open {onOpenChange}>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				{variant}
				size="sm"
				aria-label={ariaLabel}
				class={cn('toolbar-trigger gap-1.5 px-2.5 font-medium', className)}
			>
				{@render icon?.()}
				<span>{label}</span>
				{#if value}<span class="text-muted-foreground truncate font-normal">{value}</span>{/if}
				<ChevronDown class="text-muted-foreground size-3.5" />
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content
		bind:ref={content}
		{align}
		sideOffset={6}
		role="dialog"
		aria-label={dialogLabel}
		onOpenAutoFocus={(event) => {
			if (!focusContentOnOpen) return;
			event.preventDefault();
			content?.focus();
		}}
		class={cn(
			'max-h-[calc(100dvh-6rem)] w-auto max-w-[calc(100vw-1.5rem)] gap-0 overflow-y-auto p-0',
			contentClass
		)}
	>
		{@render children()}
	</Popover.Content>
</Popover.Root>
