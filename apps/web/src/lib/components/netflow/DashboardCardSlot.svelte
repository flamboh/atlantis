<script lang="ts">
	import { tick, type Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';
	import { setCardFrame } from '#lib/components/charts/card-frame.ts';

	let {
		id,
		title,
		first,
		last,
		resizable,
		onMove,
		onResize,
		children,
		...rest
	}: HTMLAttributes<HTMLElement> & {
		id: string;
		title: string;
		first: boolean;
		last: boolean;
		resizable: boolean;
		onMove: (offset: number) => void;
		onResize: (offset: number) => void;
		children: Snippet;
	} = $props();

	let section = $state<HTMLElement | null>(null);

	// The menu that triggered a move can unmount when a placeholder card activates, so the slot
	// restores focus to whichever menu trigger the card renders once the list has settled.
	async function moveAndRefocus(offset: number) {
		onMove(offset);
		await tick();
		requestAnimationFrame(() =>
			section?.querySelector<HTMLElement>('[data-card-menu-trigger]')?.focus()
		);
	}

	setCardFrame({
		get id() {
			return id;
		},
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
		move: (offset) => void moveAndRefocus(offset),
		resize: (offset) => onResize(offset)
	});
</script>

<section bind:this={section} role="listitem" data-chart-card data-chart-id={id} {...rest}>
	{@render children()}
</section>
