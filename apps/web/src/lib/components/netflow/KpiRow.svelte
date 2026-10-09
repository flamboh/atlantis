<script lang="ts">
	import * as Card from '#lib/components/ui/card/index.ts';
	import { Skeleton } from '#lib/components/ui/skeleton/index.ts';
	import type { NetflowStatsData } from './netflow-stats-data.svelte.ts';
	import { sumNetflowWindow } from './netflow-window-totals.ts';

	let { stats }: { stats: NetflowStatsData } = $props();

	const compact = new Intl.NumberFormat('en-US', {
		notation: 'compact',
		maximumFractionDigits: 2
	});
	const decimal = new Intl.NumberFormat('en-US', { maximumFractionDigits: 1 });

	function formatBytes(value: number): string {
		for (const [power, unit] of [
			[5, 'PB'],
			[4, 'TB'],
			[3, 'GB'],
			[2, 'MB'],
			[1, 'KB']
		] as const) {
			if (value >= 1024 ** power) return `${(value / 1024 ** power).toFixed(2)} ${unit}`;
		}
		return `${decimal.format(value)} B`;
	}

	const totals = $derived(sumNetflowWindow(stats.results));
	const ready = $derived(!stats.loading && !stats.error);
	const kpis = $derived([
		{
			id: 'flows',
			label: 'Flows',
			value: compact.format(totals.flows),
			exact: totals.flows,
			detail: totals.flows
				? `${decimal.format((totals.flowsTcp / totals.flows) * 100)}% TCP`
				: 'No flows'
		},
		{
			id: 'packets',
			label: 'Packets',
			value: compact.format(totals.packets),
			exact: totals.packets,
			detail: totals.flows
				? `${decimal.format(totals.packets / totals.flows)} per flow`
				: 'No packets'
		},
		{
			id: 'bytes',
			label: 'Bytes',
			value: formatBytes(totals.bytes),
			exact: totals.bytes,
			detail: totals.packets
				? `${formatBytes(totals.bytes / totals.packets)} per packet`
				: 'No bytes'
		},
		{
			id: 'buckets',
			label: 'Complete buckets',
			value: `${totals.completeBuckets.toLocaleString()} / ${totals.buckets.toLocaleString()}`,
			exact: totals.completeBuckets,
			detail:
				[
					totals.partialBuckets && `${totals.partialBuckets.toLocaleString()} partial`,
					totals.unknownBuckets && `${totals.unknownBuckets.toLocaleString()} missing`
				]
					.filter(Boolean)
					.join(' · ') || 'Full source coverage'
		}
	]);
</script>

<section aria-label="Window totals" class="grid grid-cols-2 gap-3 lg:grid-cols-4">
	{#each kpis as kpi (kpi.id)}
		<Card.Root size="sm" class="gap-1 px-4 py-3" data-kpi={kpi.id}>
			<p class="text-muted-foreground text-xs font-medium">{kpi.label}</p>
			{#if ready}
				<p
					class="text-foreground truncate text-xl font-semibold tracking-tight tabular-nums sm:text-2xl"
					title={kpi.exact.toLocaleString()}
					data-kpi-value={kpi.exact}
				>
					{kpi.value}
				</p>
				<p class="text-muted-foreground truncate text-xs">{kpi.detail}</p>
			{:else if stats.loading}
				<Skeleton class="my-1 h-7 w-24" />
				<Skeleton class="h-3.5 w-20" />
			{:else}
				<p class="text-muted-foreground text-xl font-semibold sm:text-2xl">—</p>
				<p class="text-muted-foreground truncate text-xs">Unavailable</p>
			{/if}
		</Card.Root>
	{/each}
</section>
