<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';
	import { setCardFrame } from '#lib/components/charts/card-frame.ts';

	let {
		title,
		first,
		last,
		resizable,
		onMove,
		onResize,
		children,
		...rest
	}: HTMLAttributes<HTMLElement> & {
		title: string;
		first: boolean;
		last: boolean;
		resizable: boolean;
		onMove: (offset: number) => void;
		onResize: (offset: number) => void;
		children: Snippet;
	} = $props();

	setCardFrame({
		get title() {
			return title;
		},
		get first() {
			return first;
		},
		get last() {
			return last;
		},
		get resizable() {
			return resizable;
		},
		move: (offset) => onMove(offset),
		resize: (offset) => onResize(offset)
	});
</script>

<section role="listitem" data-chart-card {...rest}>
	{@render children()}
</section>
