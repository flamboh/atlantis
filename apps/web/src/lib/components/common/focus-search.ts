import type { Attachment } from 'svelte/attachments';

export const focusSearch: Attachment<HTMLElement> = (node) => {
	const frame = requestAnimationFrame(() => {
		const content = node.closest<HTMLElement>('[data-slot="popover-content"]');
		if (content && content === node.ownerDocument.activeElement) {
			node.querySelector<HTMLInputElement>('input')?.focus();
		}
	});
	return () => cancelAnimationFrame(frame);
};
