<script lang="ts">
	import { buildNetflowFileHref } from '$lib/utils/netflow-file-navigation';
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

<div class="mb-2 flex items-center justify-between text-2xl text-gray-900 dark:text-gray-100">
	<h1>NetFlow File: {filename}</h1>
	<form method="GET" action={buildNetflowFileHref(nextSlug, dataset, flowScope)}>
		<button
			type="submit"
			class="w-24 rounded bg-blue-600 px-4 py-1 text-center text-sm text-white hover:bg-blue-700"
		>
			Next File
		</button>
	</form>
</div>

<div class="dark:border-dark-border dark:bg-dark-subtle mb-2 rounded-lg border bg-blue-100 p-4">
	<h2 class="mb-2 text-lg font-semibold dark:text-gray-100">File Information</h2>
	<div class="grid grid-cols-3 gap-2 dark:text-gray-300">
		<div>Date: {year}-{month}-{day}</div>
		<div>Time: {hour}:{minute}</div>
		<div>Processed in DB: {processedAt}</div>
	</div>
</div>
