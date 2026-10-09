<script lang="ts">
	import ChartLoading from '#lib/components/charts/ChartLoading.svelte';
	import ChartCardHeader from './ChartCardHeader.svelte';
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
	<ChartCardHeader {title} {subtitle} controls={unavailableCopy ? undefined : controls} />

	<Card.Content class="p-0">
		{#if unavailableCopy}
			<div
				class="text-muted-foreground flex min-h-40 items-center justify-center px-6 py-8 text-center text-sm"
				data-testid="chart-unavailable"
			>
				{unavailableCopy}
			</div>
		{:else}
			<div
				class={size === 'spectrum'
					? 'chart-frame relative h-[400px] min-h-[300px] resize-y overflow-hidden p-3 [--chart-height:400px]'
					: size === 'split'
						? 'chart-frame relative h-[640px] min-h-[520px] resize-y overflow-hidden p-3 [--chart-height:640px] xl:h-[320px] xl:min-h-[240px] xl:[--chart-height:320px]'
						: 'chart-frame relative h-[320px] min-h-[240px] resize-y overflow-hidden p-3'}
				role="presentation"
			>
				{#if selectionUnavailableCopy}
					<div
						class="text-muted-foreground flex h-full items-center justify-center px-6 text-center text-sm"
						data-testid="chart-selection-unavailable"
					>
						{selectionUnavailableCopy}
					</div>
				{:else if loading}
					<ChartLoading label={loadingCopy} />
				{:else if error}
					<div class="text-destructive flex h-full items-center justify-center text-sm">
						{error}
					</div>
				{:else if noMetrics}
					<div class="text-muted-foreground flex h-full items-center justify-center text-sm">
						{noMetricsCopy}
					</div>
				{:else if empty}
					<div class="text-muted-foreground flex h-full items-center justify-center text-sm">
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
