<script lang="ts">
	let {
		filename,
		year,
		month,
		day,
		hour,
		minute,
		processedAt
	}: {
		filename: string;
		year: string;
		month: string;
		day: string;
		hour: string;
		minute: string;
		processedAt: string;
	} = $props();
</script>

<div class="file-context">
	<h2 class="rail-heading">File Information</h2>
	<p class="text-muted-foreground text-xs">Filename</p>
	<p class="font-mono text-xs break-all">{filename}</p>
	<p>Date: {year}-{month}-{day}</p>
	<p>Time: {hour}:{minute}</p>
	<p class="text-xs">Processed in DB: {processedAt}</p>
</div>
