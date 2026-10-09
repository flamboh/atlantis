<script lang="ts" module>
	export type ToolbarSelectOption<T extends string> = {
		value: T;
		label: string;
		description?: string;
		disabled?: boolean;
	};
</script>

<script lang="ts" generics="T extends string">
	import * as Select from '#lib/components/ui/select/index.ts';
	import { cn } from '#lib/utils.ts';

	let {
		label,
		value,
		options,
		onValueChange,
		variant = 'outline',
		class: className
	}: {
		label: string;
		value: T;
		options: readonly ToolbarSelectOption<T>[];
		onValueChange: (value: T) => void;
		variant?: 'outline' | 'ghost';
		class?: string;
	} = $props();

	const selected = $derived(options.find((option) => option.value === value));
</script>

<Select.Root
	type="single"
	{value}
	onValueChange={(next) => {
		if (next && next !== value) onValueChange(next as T);
	}}
	items={options.map((option) => ({
		value: option.value,
		label: option.label,
		disabled: option.disabled
	}))}
>
	<Select.Trigger
		size="sm"
		aria-label={`${label}: ${selected?.label ?? value}`}
		class={cn(
			'toolbar-trigger h-8 gap-1.5 px-2.5 font-medium shadow-xs',
			variant === 'outline'
				? 'border-border bg-background hover:bg-muted dark:bg-input/30 dark:hover:bg-input/50'
				: 'hover:bg-muted border-transparent bg-transparent shadow-none',
			className
		)}
	>
		<span>{label}</span>
		<span class="text-muted-foreground font-normal">{selected?.label ?? value}</span>
	</Select.Trigger>
	<Select.Content align="start" class="min-w-44">
		{#each options as option (option.value)}
			<Select.Item value={option.value} label={option.label} disabled={option.disabled}>
				<span class="flex flex-col gap-0.5 whitespace-normal">
					<span>{option.label}</span>
					{#if option.description}<span class="text-muted-foreground text-xs"
							>{option.description}</span
						>{/if}
				</span>
			</Select.Item>
		{/each}
	</Select.Content>
</Select.Root>
