<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';

	let {
		message,
		tone = 'neutral',
		actionLabel,
		action
	}: {
		message: string;
		tone?: 'neutral' | 'danger';
		actionLabel?: string;
		action?: () => void;
	} = $props();

	const classes = $derived(
		tone === 'danger'
			? 'bg-destructive/5 text-destructive ring-destructive/20'
			: 'text-muted-foreground'
	);
</script>

<Card.Root size="sm" class={classes}>
	<Card.Content>
		<p>{message}</p>
		{#if action && actionLabel}
			<Button type="button" variant="destructive" size="sm" class="mt-2" onclick={action}>
				{actionLabel}
			</Button>
		{/if}
	</Card.Content>
</Card.Root>
