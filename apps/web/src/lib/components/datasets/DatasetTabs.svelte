<script lang="ts">
	import { resolve } from '$app/paths';
	import {
		segmentedControlActiveClass,
		segmentedControlGroupClass,
		segmentedControlInactiveClass,
		segmentedControlItemClass
	} from '$lib/components/common/SegmentedControl.svelte';
	import { cn } from '$lib/utils';

	let { datasetId, active }: { datasetId: string; active: 'dashboard' | 'alerts' } = $props();
</script>

<nav aria-label="Dataset views" class={cn(segmentedControlGroupClass, 'grid-cols-2')}>
	<a
		href={resolve('/datasets/[dataset]', { dataset: datasetId })}
		class={cn(
			segmentedControlItemClass,
			active === 'dashboard' ? segmentedControlActiveClass : segmentedControlInactiveClass
		)}
		aria-current={active === 'dashboard' ? 'page' : undefined}>Dashboard</a
	>
	<a
		href={resolve('/datasets/[dataset]/alerts', { dataset: datasetId })}
		class={cn(
			segmentedControlItemClass,
			active === 'alerts' ? segmentedControlActiveClass : segmentedControlInactiveClass
		)}
		aria-current={active === 'alerts' ? 'page' : undefined}>Alerts</a
	>
</nav>
