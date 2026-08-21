<script lang="ts">
	import { buildNetflowFileHref } from '$lib/utils/netflow-file-navigation';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import type { FlowScope } from '$lib/types/types';

	let {
		dataset,
		nextSlug,
		flowScope,
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
		flowScope: FlowScope;
		filename: string;
		year: string;
		month: string;
		day: string;
		hour: string;
		minute: string;
		processedAt: string;
	} = $props();
</script>

<div class="text-foreground mb-2 flex items-center justify-between text-2xl">
	<h1>NetFlow File: {filename}</h1>
	<form method="GET" action={buildNetflowFileHref(nextSlug, dataset, flowScope)}>
		<Button type="submit" size="sm" class="w-24">Next File</Button>
	</form>
</div>

<Card.Root size="sm" class="mb-2">
	<Card.Header>
		<Card.Title class="text-lg font-semibold"><h2>File Information</h2></Card.Title>
	</Card.Header>
	<Card.Content class="grid grid-cols-3 gap-2">
		<div>Date: {year}-{month}-{day}</div>
		<div>Time: {hour}:{minute}</div>
		<div>Processed in DB: {processedAt}</div>
	</Card.Content>
</Card.Root>
