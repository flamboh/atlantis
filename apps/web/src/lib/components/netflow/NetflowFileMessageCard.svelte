<script lang="ts">
	import { Button } from '#lib/components/ui/button/index.ts';
	import * as Card from '#lib/components/ui/card/index.ts';

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
			? 'border-destructive/30 bg-destructive/5 text-destructive'
			: 'text-muted-foreground'
	);
</script>

<Card.Root size="sm" class={classes}>
	<Card.Content>
		<p>{message}</p>
		{#if action && actionLabel}
			<Button type="button" variant="outline" size="sm" class="mt-3" onclick={action}>
				{actionLabel}
			</Button>
		{/if}
	</Card.Content>
</Card.Root>
