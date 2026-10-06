<script lang="ts">
	import { buildNetflowFileHref } from '#lib/utils/netflow-file-navigation.ts';
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Card from '#lib/components/ui/card/index.ts';
	import type { FlowDirection, MaadIpVersion, MaadMeasure } from '#lib/types/types.ts';

	let {
		dataset,
		nextSlug,
		direction,
		ipVersion,
		measure,
		filename,
		year,
		month,
		day,
		hour,
		minute,
		processedAt
	}: {
		dataset: string;
		nextSlug: string;
		direction: FlowDirection;
		ipVersion: MaadIpVersion;
		measure: MaadMeasure;
		filename: string;
		year: string;
		month: string;
		day: string;
		hour: string;
		minute: string;
		processedAt: string;
	} = $props();
</script>

<div class="mb-4 flex flex-col items-start justify-between gap-3 sm:flex-row sm:items-center">
	<h1 class="page-heading min-w-0">
		<span class="page-eyebrow block">NetFlow File: </span><span class="font-mono text-lg sm:text-xl"
			>{filename}</span
		>
	</h1>
	<Button
		href={buildNetflowFileHref(nextSlug, dataset, direction, ipVersion, measure)}
		size="sm"
		class="shrink-0"
	>
		Next File
	</Button>
</div>

<Card.Root size="sm" class="mb-2">
	<Card.Header>
		<Card.Title class="text-lg font-semibold"><h2>File Information</h2></Card.Title>
	</Card.Header>
	<Card.Content class="grid grid-cols-1 gap-3 sm:grid-cols-3">
		<div>Date: {year}-{month}-{day}</div>
		<div>Time: {hour}:{minute}</div>
		<div class="min-w-0 break-words">Processed in DB: {processedAt}</div>
	</Card.Content>
</Card.Root>
