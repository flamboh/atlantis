<script lang="ts">
	import type { FileDetailResourceView, NetflowFileRouterRow } from './file-detail-loader.svelte';
	import type { FileIpCounts } from '#lib/types/types.ts';
	import { Badge } from '#lib/components/ui/badge/index.ts';
	import * as Table from '#lib/components/ui/table/index.ts';

	let {
		row,
		formatCount,
		formatTimestampAsPST
	}: {
		row: NetflowFileRouterRow;
		formatCount: (value: number | null | undefined) => string;
		formatTimestampAsPST: (timestamp: number) => string;
	} = $props();

	function formatIpCount(slot: FileDetailResourceView<FileIpCounts>, family: 'ipv4' | 'ipv6') {
		const value = family === 'ipv4' ? slot.data?.ipv4Count : slot.data?.ipv6Count;
		if (value !== null && value !== undefined) {
			return formatCount(value);
		}

		return slot.loading ? '...' : 'N/A';
	}

	function formatOptionalTimestamp(timestamp: number | null | undefined) {
		if (typeof timestamp !== 'number' || !Number.isFinite(timestamp)) {
			return 'N/A';
		}

		return formatTimestampAsPST(timestamp * 1000);
	}

	const summary = $derived(row.summary);
	const metricRows = $derived([
		{
			label: 'Flows',
			values: [
				summary.flows,
				summary.flows_tcp,
				summary.flows_udp,
				summary.flows_icmp,
				summary.flows_other
			]
		},
		{
			label: 'Packets',
			values: [
				summary.packets,
				summary.packets_tcp,
				summary.packets_udp,
				summary.packets_icmp,
				summary.packets_other
			]
		},
		{
			label: 'Bytes',
			values: [
				summary.bytes,
				summary.bytes_tcp,
				summary.bytes_udp,
				summary.bytes_icmp,
				summary.bytes_other
			]
		}
	]);
	const details = $derived([
		['Bucket', formatOptionalTimestamp(summary.bucket_start)],
		['First', formatOptionalTimestamp(summary.first_timestamp)],
		['Last', formatOptionalTimestamp(summary.last_timestamp)],
		['First ms', formatCount(summary.msec_first)],
		['Last ms', formatCount(summary.msec_last)],
		['Seq failures', formatCount(summary.sequence_failures)]
	]);
</script>

<div class="flex min-w-0 flex-col gap-4 p-4">
	<div class="min-w-0">
		<div class="flex flex-wrap items-center gap-2">
			<h3 class="text-base font-semibold tracking-tight">Source: {row.router}</h3>
			<Badge variant="outline" class="text-muted-foreground font-normal"
				>Kind {summary.input_kind ?? 'unknown'}</Badge
			>
			<Badge
				variant={summary.input_status === 'failed' ? 'destructive' : 'outline'}
				class="font-normal">Status {summary.input_status ?? 'unknown'}</Badge
			>
			<Badge variant="outline" class="text-muted-foreground font-normal"
				>{summary.file_exists_on_disk ? 'on disk' : 'not on disk'}</Badge
			>
		</div>
		<p class="text-muted-foreground mt-1 font-mono text-xs break-all">
			{summary.file_path ?? 'No input locator recorded'}
		</p>
	</div>
	{#if summary.input_error_message}
		<p
			class="border-destructive/30 bg-destructive/5 text-destructive rounded-md border px-3 py-2 text-sm"
		>
			{summary.input_error_message}
		</p>
	{/if}

	<div class="overflow-x-auto rounded-md border">
		<Table.Root class="text-xs">
			<Table.Header class="bg-muted/50">
				<Table.Row class="hover:bg-transparent">
					<Table.Head class="h-8 px-2.5"><span class="sr-only">Metric</span></Table.Head>
					{#each ['Total', 'TCP', 'UDP', 'ICMP', 'Other'] as column (column)}
						<Table.Head class="text-muted-foreground h-8 px-2.5 text-right text-xs"
							>{column}</Table.Head
						>
					{/each}
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each metricRows as metric (metric.label)}
					<Table.Row class="hover:bg-transparent">
						<Table.Head scope="row" class="text-foreground h-8 px-2.5 font-medium"
							>{metric.label}</Table.Head
						>
						{#each metric.values as value, index (index)}
							<Table.Cell class="px-2.5 py-1.5 text-right tabular-nums"
								>{value.toLocaleString()}</Table.Cell
							>
						{/each}
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>

	<dl class="grid grid-cols-2 gap-x-4 gap-y-2 text-xs sm:grid-cols-4 2xl:grid-cols-2">
		<div>
			<dt class="text-muted-foreground">Unique source IPs</dt>
			<dd class="tabular-nums">
				IPv4 {formatIpCount(row.source.ipCounts, 'ipv4')} · IPv6 {formatIpCount(
					row.source.ipCounts,
					'ipv6'
				)}
			</dd>
		</div>
		<div>
			<dt class="text-muted-foreground">Unique destination IPs</dt>
			<dd class="tabular-nums">
				IPv4 {formatIpCount(row.destination.ipCounts, 'ipv4')} · IPv6 {formatIpCount(
					row.destination.ipCounts,
					'ipv6'
				)}
			</dd>
		</div>
		{#each details as [label, value] (label)}
			<div>
				<dt class="text-muted-foreground">{label}</dt>
				<dd class="tabular-nums">{value}</dd>
			</div>
		{/each}
	</dl>
</div>
