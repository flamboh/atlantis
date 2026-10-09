<script lang="ts" module>
	export type SegmentedOption<T extends string> = {
		value: T;
		label: string;
		title?: string;
		disabled?: boolean;
	};
</script>

<script lang="ts" generics="T extends string">
	import * as ToggleGroup from '#lib/components/ui/toggle-group/index.ts';
	import { cn } from '#lib/utils.ts';

	let {
		options,
		value,
		onValueChange,
		ariaLabel,
		class: className,
		itemClass
	}: {
		options: readonly SegmentedOption<T>[];
		value: T | null;
		onValueChange?: (value: T) => void;
		ariaLabel: string;
		class?: string;
		itemClass?: string;
	} = $props();
</script>

<ToggleGroup.Root
	type="single"
	value={value ?? ''}
	onValueChange={(next) => {
		if (next && next !== value) onValueChange?.(next as T);
	}}
	spacing={1}
	aria-label={ariaLabel}
	class={cn('segmented bg-muted gap-0.5 rounded-md p-0.5', className)}
>
	{#each options as option (option.value)}
		<ToggleGroup.Item
			value={option.value}
			disabled={option.disabled}
			title={option.title}
			class={cn(
				'text-muted-foreground hover:text-foreground data-[state=on]:bg-card data-[state=on]:text-foreground h-7 min-w-0 rounded-[calc(var(--radius)-2px)] px-2.5 text-xs font-medium hover:bg-transparent data-[state=on]:shadow-xs dark:data-[state=on]:bg-[#2a2a2a]',
				itemClass
			)}>{option.label}</ToggleGroup.Item
		>
	{/each}
</ToggleGroup.Root>
