import { afterEach, describe, expect, it, vi } from 'vitest';
import { createNearViewportAttachment } from '../../src/lib/components/netflow/near-viewport';

class FakeIntersectionObserver {
	static instances: FakeIntersectionObserver[] = [];

	readonly observed = new Set<Element>();
	disconnected = false;

	constructor(
		private readonly callback: IntersectionObserverCallback,
		readonly options?: IntersectionObserverInit
	) {
		FakeIntersectionObserver.instances.push(this);
	}

	observe(target: Element) {
		this.observed.add(target);
	}

	unobserve(target: Element) {
		this.observed.delete(target);
	}

	disconnect() {
		this.disconnected = true;
		this.observed.clear();
	}

	takeRecords(): IntersectionObserverEntry[] {
		return [];
	}

	trigger(target: Element, isIntersecting: boolean) {
		this.callback(
			[
				{
					target,
					isIntersecting,
					intersectionRatio: isIntersecting ? 1 : 0
				} as IntersectionObserverEntry
			],
			this as unknown as IntersectionObserver
		);
	}
}

afterEach(() => {
	vi.unstubAllGlobals();
	FakeIntersectionObserver.instances = [];
});

describe('createNearViewportAttachment', () => {
	it('activates once when the card enters the near-viewport margin', () => {
		vi.stubGlobal('IntersectionObserver', FakeIntersectionObserver);
		const activate = vi.fn();
		const attachment = createNearViewportAttachment(activate);
		const element = new EventTarget() as HTMLElement;

		const cleanup = attachment(element);
		const observer = FakeIntersectionObserver.instances[0];
		expect(observer?.options).toEqual({ rootMargin: '128px 0px' });
		expect(observer?.observed.has(element)).toBe(true);

		observer?.trigger(element, false);
		expect(activate).not.toHaveBeenCalled();

		observer?.trigger(element, true);
		observer?.trigger(element, true);
		expect(activate).toHaveBeenCalledOnce();
		expect(observer?.disconnected).toBe(true);

		cleanup?.();
	});

	it('disconnects without activating when the card is removed', () => {
		vi.stubGlobal('IntersectionObserver', FakeIntersectionObserver);
		const activate = vi.fn();
		const attachment = createNearViewportAttachment(activate);
		const element = new EventTarget() as HTMLElement;

		const cleanup = attachment(element);
		cleanup?.();

		expect(activate).not.toHaveBeenCalled();
		expect(FakeIntersectionObserver.instances[0]?.disconnected).toBe(true);
	});

	it('activates immediately when IntersectionObserver is unavailable', () => {
		vi.stubGlobal('IntersectionObserver', undefined);
		const activate = vi.fn();
		const attachment = createNearViewportAttachment(activate);

		attachment(new EventTarget() as HTMLElement);

		expect(activate).toHaveBeenCalledOnce();
	});
});
