<script lang="ts" module>
	export type SegmentedControlOption<T extends string> = {
		value: T;
		label: string;
		disabled?: boolean;
		disabledReason?: string;
		title?: string;
	};

	export const segmentedControlGroupClass =
		'grid w-full gap-0.5 rounded-md border border-border bg-muted p-1 sm:w-fit';
	export const segmentedControlItemClass =
		'flex h-auto min-h-7 w-full items-center justify-center rounded px-2.5 py-0.5 text-center text-xs font-medium transition-colors focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none';
	export const segmentedControlActiveClass =
		'bg-primary text-primary-foreground shadow-sm hover:bg-primary/90 hover:text-primary-foreground';
	export const segmentedControlInactiveClass =
		'text-muted-foreground hover:bg-background/60 hover:text-foreground';
</script>

<script lang="ts" generics="T extends string">
	import { Button } from '$lib/components/ui/button';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { cn } from '$lib/utils';

	interface Props {
		options: readonly SegmentedControlOption<T>[];
		value: T | null;
		onValueChange?: (value: T) => void;
		class?: string;
		style?: string;
		buttonClass?: string;
		ariaLabel?: string;
	}

	let {
		options,
		value,
		onValueChange,
		class: className,
		style,
		buttonClass,
		ariaLabel
	}: Props = $props();

	function selectOption(option: SegmentedControlOption<T>) {
		if (!option.disabled && !option.disabledReason && option.value !== value) {
			onValueChange?.(option.value);
		}
	}

	function optionClass(option: SegmentedControlOption<T>) {
		return cn(
			segmentedControlItemClass,
			option.value === value ? segmentedControlActiveClass : segmentedControlInactiveClass,
			(option.disabled || option.disabledReason) && 'cursor-not-allowed opacity-50',
			buttonClass
		);
	}
</script>

<div class={cn(segmentedControlGroupClass, className)} {style} role="group" aria-label={ariaLabel}>
	{#each options as option (option.value)}
		{#if option.disabledReason}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<Button
							{...props}
							variant="ghost"
							class={optionClass(option)}
							onclick={() => selectOption(option)}
							aria-pressed={option.value === value}
							aria-disabled="true"
							title={option.title}
						>
							{option.label}
						</Button>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content sideOffset={4}>{option.disabledReason}</Tooltip.Content>
			</Tooltip.Root>
		{:else}
			<Button
				variant="ghost"
				class={optionClass(option)}
				onclick={() => selectOption(option)}
				aria-pressed={option.value === value}
				disabled={option.disabled}
				title={option.title}
			>
				{option.label}
			</Button>
		{/if}
	{/each}
</div>
