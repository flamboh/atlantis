import type { Attachment } from 'svelte/attachments';

/** Lazily loaded popover lists mount after the popover has placed focus, so move it to the search. */
export const focusSearch: Attachment<HTMLElement> = (node) => {
	node.querySelector<HTMLInputElement>('input')?.focus();
};
