import type { Attachment } from 'svelte/attachments';

export const NEAR_VIEWPORT_ROOT_MARGIN = '128px 0px';

/** Mounts expensive content once its container is close enough to be useful. */
export function createNearViewportAttachment(onActivate: () => void): Attachment<HTMLElement> {
	return (element) => {
		if (typeof IntersectionObserver === 'undefined') {
			onActivate();
			return;
		}

		let activated = false;
		const observer = new IntersectionObserver(
			(entries) => {
				if (
					activated ||
					!entries.some((entry) => entry.target === element && entry.isIntersecting)
				) {
					return;
				}
				activated = true;
				observer.disconnect();
				onActivate();
			},
			{ rootMargin: NEAR_VIEWPORT_ROOT_MARGIN }
		);
		observer.observe(element);

		return () => observer.disconnect();
	};
}
