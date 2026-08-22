<script lang="ts">
	import DragGrip from '$lib/components/common/DragGrip.svelte';
	import * as Card from '$lib/components/ui/card';
	import type { Snippet } from 'svelte';

	let {
		title,
		size = 'default',
		loading,
		error,
		noMetrics,
		empty,
		loadingCopy,
		noMetricsCopy,
		emptyCopy,
		isDraggingRange,
		selectionLeft,
		selectionWidth,
		selectionTop,
		selectionHeight,
		mirroredSelectionStyle,
		minDragPixels,
		controls,
		children,
		overlay,
		onmousedown,
		onmousemove,
		onmouseup,
		onmouseleave
	}: {
		title: string;
		size?: 'default' | 'spectrum';
		loading: boolean;
		error: string | null;
		noMetrics: boolean;
		empty: boolean;
		loadingCopy: string;
		noMetricsCopy: string;
		emptyCopy: string;
		isDraggingRange: boolean;
		selectionLeft: number;
		selectionWidth: number;
		selectionTop: number;
		selectionHeight: number;
		mirroredSelectionStyle: string | null;
		minDragPixels: number;
		controls?: Snippet;
		children: Snippet;
		overlay?: Snippet;
		onmousedown: (event: MouseEvent) => void;
		onmousemove: (event: MouseEvent) => void;
		onmouseup: () => void;
		onmouseleave: () => void;
	} = $props();
</script>

<Card.Root size="sm" class="gap-0 py-0">
	<Card.Header
		class="border-border relative cursor-grab border-b py-4 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<Card.Title class="text-lg font-semibold"><h2>{title}</h2></Card.Title>
		<DragGrip />
	</Card.Header>

	<Card.Content class="space-y-4 py-4">
		{@render controls?.()}

		<div
			class={size === 'spectrum'
				? 'border-border bg-background/60 relative h-[400px] min-h-[300px] resize-y overflow-hidden rounded-md border'
				: 'border-border bg-background/60 relative h-[320px] min-h-[240px] resize-y overflow-hidden rounded-md border'}
			role="presentation"
			{onmousedown}
			{onmousemove}
			{onmouseup}
			{onmouseleave}
		>
			{#if loading}
				<div class="text-muted-foreground flex h-full items-center justify-center">
					{loadingCopy}
				</div>
			{:else if error}
				<div class="text-destructive flex h-full items-center justify-center">{error}</div>
			{:else if noMetrics}
				<div class="text-muted-foreground flex h-full items-center justify-center">
					{noMetricsCopy}
				</div>
			{:else if empty}
				<div class="text-muted-foreground flex h-full items-center justify-center">
					{emptyCopy}
				</div>
			{:else}
				<div class="relative h-full">
					{@render children()}
					{@render overlay?.()}
					{#if isDraggingRange && selectionWidth >= minDragPixels}
						<div
							class="border-muted-foreground/70 bg-muted-foreground/20 pointer-events-none absolute border"
							style={`left:${selectionLeft}px; width:${selectionWidth}px; top:${selectionTop}px; height:${selectionHeight}px;`}
						></div>
					{/if}
					{#if !isDraggingRange && mirroredSelectionStyle !== null}
						<div
							class="border-muted-foreground/70 bg-muted-foreground/20 pointer-events-none absolute border"
							style={mirroredSelectionStyle}
						></div>
					{/if}
				</div>
			{/if}
		</div>
	</Card.Content>
</Card.Root>
