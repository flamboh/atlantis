<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { Skeleton } from '$lib/components/ui/skeleton';

	let { count }: { count: number } = $props();
	const skeletons = $derived(Array.from({ length: count }, (_, index) => index));
</script>

<div class="space-y-2">
	{#each skeletons as skeletonId (`summary-skeleton-${skeletonId}`)}
		<Card.Root size="sm" class="gap-0 py-0">
			<Card.Header class="bg-primary py-4">
				<Skeleton class="bg-primary-foreground/70 mb-2 h-7 w-56" />
				<Skeleton class="bg-primary-foreground/60 mb-4 h-5 w-[34rem] max-w-full" />
				<div class="grid grid-cols-1 gap-2 text-sm md:grid-cols-2">
					<div>
						<Skeleton class="bg-primary-foreground/60 mb-2 h-5 w-40" />
						<Skeleton class="bg-primary-foreground/60 mb-1 h-4 w-20" />
						<Skeleton class="bg-primary-foreground/60 h-4 w-20" />
					</div>
					<div>
						<Skeleton class="bg-primary-foreground/60 mb-2 h-5 w-44" />
						<Skeleton class="bg-primary-foreground/60 mb-1 h-4 w-20" />
						<Skeleton class="bg-primary-foreground/60 h-4 w-20" />
					</div>
				</div>
				<div class="mt-4 grid grid-cols-4 gap-4 text-sm">
					{#each [0, 1, 2, 3] as columnIndex (`summary-column-${skeletonId}-${columnIndex}`)}
						<div>
							<Skeleton class="bg-primary-foreground/60 mb-2 h-5 w-28" />
							<Skeleton class="bg-primary-foreground/60 mb-1 h-4 w-24" />
							<Skeleton class="bg-primary-foreground/60 mb-1 h-4 w-20" />
							<Skeleton class="bg-primary-foreground/60 mb-1 h-4 w-24" />
							<Skeleton class="bg-primary-foreground/60 h-4 w-20" />
						</div>
					{/each}
				</div>
			</Card.Header>
			<Card.Content class="space-y-4 py-4">
				<Skeleton class="h-6 w-36" />
				<div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
					<div class="space-y-3">
						<Skeleton class="h-5 w-44" />
						<Skeleton class="h-4 w-96 max-w-full" />
						<Skeleton class="h-96" />
					</div>
					<div class="space-y-3">
						<Skeleton class="h-5 w-52" />
						<Skeleton class="h-4 w-96 max-w-full" />
						<Skeleton class="h-96" />
					</div>
				</div>
				<div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
					<div class="space-y-3">
						<Skeleton class="h-5 w-44" />
						<Skeleton class="h-4 w-96 max-w-full" />
						<Skeleton class="h-96" />
					</div>
					<div class="space-y-3">
						<Skeleton class="h-5 w-52" />
						<Skeleton class="h-4 w-96 max-w-full" />
						<Skeleton class="h-96" />
					</div>
				</div>
				<div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
					<div class="space-y-3">
						<Skeleton class="h-5 w-48" />
						<Skeleton class="h-56" />
					</div>
					<div class="space-y-3">
						<Skeleton class="h-5 w-56" />
						<Skeleton class="h-56" />
					</div>
				</div>
			</Card.Content>
		</Card.Root>
	{/each}
</div>
