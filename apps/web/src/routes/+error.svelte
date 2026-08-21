<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button';
	import { Card, CardContent } from '$lib/components/ui/card';
	import { t } from '$lib/i18n';

	function getErrorTitle(status: number): string {
		switch (status) {
			case 400:
				return 'Bad Request';
			case 404:
				return 'Page Not Found';
			case 500:
				return 'Internal Server Error';
			default:
				return 'Error';
		}
	}
</script>

<div class="container mx-auto p-6">
	<div class="mx-auto max-w-2xl text-center">
		<div class="mb-6">
			<h1 class="text-foreground mb-2 text-4xl font-bold">{page.status}</h1>
			<h2 class="text-foreground mb-4 text-2xl font-semibold">{getErrorTitle(page.status)}</h2>
		</div>

		{#if page.error?.message}
			<Card class="border-destructive bg-destructive/10 text-destructive mb-6 gap-0 py-6 ring-0">
				<CardContent class="text-sm">
					{page.error.message}
				</CardContent>
			</Card>
		{/if}

		<div class="space-y-3">
			<div class="flex justify-center space-x-4">
				<Button variant="outline" onclick={() => window.history.back()}>Go Back</Button>
				<Button href={resolve('/')}>Home</Button>
			</div>

			{#if page.status === 404}
				<div class="text-muted-foreground mt-4 text-sm">
					<p>
						{t('error.404.return_to_prefix')}
						<a href={resolve('/')} class="text-primary hover:underline">
							{t('error.404.dataset_index')}
						</a>
						{t('error.404.or_use_prefix')}
						<a href={resolve('/netflow/files')} class="text-primary hover:underline">
							{t('error.404.file_lookup')}
						</a>
						{t('error.404.timestamp_hint')}
					</p>
				</div>
			{/if}
		</div>
	</div>
</div>
