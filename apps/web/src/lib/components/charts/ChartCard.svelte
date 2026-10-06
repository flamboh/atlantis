<script lang="ts">
	import ChartLoading from '#lib/components/charts/ChartLoading.svelte';
	import DragGrip from '#lib/components/common/DragGrip.svelte';
	import * as Card from '#lib/components/ui/card/index.ts';
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = null,
		size = 'default',
		unavailableCopy = null,
		selectionUnavailableCopy = null,
		loading,
		error,
		noMetrics,
		empty,
		loadingCopy,
		noMetricsCopy,
		emptyCopy,
		controls,
		children
	}: {
		title: string;
		subtitle?: string | null;
		size?: 'default' | 'spectrum' | 'split';
		unavailableCopy?: string | null;
		selectionUnavailableCopy?: string | null;
		loading: boolean;
		error: string | null;
		noMetrics: boolean;
		empty: boolean;
		loadingCopy: string;
		noMetricsCopy: string;
		emptyCopy: string;
		controls?: Snippet;
		children: Snippet;
	} = $props();
</script>

<Card.Root
	size="sm"
	class="gap-0 py-0"
	data-testid="chart-card-state"
	data-state={unavailableCopy || selectionUnavailableCopy
		? 'unavailable'
		: loading
			? 'loading'
			: error
				? 'error'
				: noMetrics
					? 'no-metrics'
					: empty
						? 'empty'
						: 'ready'}
>
	<Card.Header
		class="border-border relative cursor-grab border-b py-4 select-none active:cursor-grabbing"
		draggable="true"
		data-drag-handle
	>
		<Card.Title class="flex items-baseline gap-3 text-lg font-semibold">
			<h2>{title}</h2>
			{#if subtitle}
				<span class="text-muted-foreground text-sm font-normal">{subtitle}</span>
			{/if}
		</Card.Title>
		<DragGrip />
	</Card.Header>

	<Card.Content class="space-y-4 py-4">
		{#if unavailableCopy}
			<div
				class="border-border bg-background/60 text-muted-foreground flex min-h-32 items-center justify-center rounded-md border px-6 py-8 text-center"
				data-testid="chart-unavailable"
			>
				{unavailableCopy}
			</div>
		{:else}
			{@render controls?.()}

			<div
				class={size === 'spectrum'
					? 'chart-frame relative h-[400px] min-h-[300px] resize-y overflow-hidden rounded-md border [--chart-height:400px]'
					: size === 'split'
						? 'chart-frame relative h-[640px] min-h-[520px] resize-y overflow-hidden rounded-md border [--chart-height:640px] xl:h-[320px] xl:min-h-[240px] xl:[--chart-height:320px]'
						: 'chart-frame relative h-[320px] min-h-[240px] resize-y overflow-hidden rounded-md border'}
				role="presentation"
			>
				{#if selectionUnavailableCopy}
					<div
						class="text-muted-foreground flex h-full items-center justify-center px-6 text-center"
						data-testid="chart-selection-unavailable"
					>
						{selectionUnavailableCopy}
					</div>
				{:else if loading}
					<ChartLoading label={loadingCopy} />
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
					</div>
				{/if}
			</div>
		{/if}
	</Card.Content>
</Card.Root>
