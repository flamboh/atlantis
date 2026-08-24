import { describe, expect, it, vi } from 'vitest';
import {
	ensureCachedWindow,
	getMissingWindowRanges,
	readCachedWindow,
	type TimeRange
} from '../../src/lib/utils/window-cache';

type Record = { bucketStart: number };

function cacheOptions(key: string, fetchRange: (range: TimeRange) => Promise<Record[]>) {
	return {
		key,
		fetchRange,
		getRecordKey: (record: Record) => `${record.bucketStart}`,
		compareRecords: (left: Record, right: Record) => left.bucketStart - right.bucketStart
	};
}

async function fillEightWindows(key: string, fetchRange: (range: TimeRange) => Promise<Record[]>) {
	for (let index = 0; index < 8; index += 1) {
		await ensureCachedWindow({
			...cacheOptions(key, fetchRange),
			requestedRange: { start: index * 10, end: (index + 1) * 10 }
		});
	}
}

describe('window cache', () => {
	it('refreshes a completed window after its maximum age', async () => {
		vi.useFakeTimers();
		try {
			vi.setSystemTime(0);
			let bucketStart = 100;
			const key = 'window-cache-max-age';
			const fetchRange = vi.fn(async () => [{ bucketStart }]);
			const options = {
				...cacheOptions(key, fetchRange),
				requestedRange: { start: 100, end: 200 },
				maxAgeMs: 1_000
			};

			await ensureCachedWindow(options);
			bucketStart = 101;
			vi.setSystemTime(999);
			await ensureCachedWindow(options);
			expect(fetchRange).toHaveBeenCalledOnce();

			vi.setSystemTime(1_000);
			await ensureCachedWindow(options);
			expect(fetchRange).toHaveBeenCalledTimes(2);
			expect(
				readCachedWindow<Record>(
					key,
					{ start: 100, end: 200 },
					(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
				)
			).toEqual([{ bucketStart: 101 }]);
		} finally {
			vi.useRealTimers();
		}
	});

	it('limits concurrent cache entries when every existing entry is in flight', async () => {
		let releaseFetches = () => {};
		const fetchGate = new Promise<void>((resolve) => {
			releaseFetches = resolve;
		});
		let startedFetches = 0;
		const requests = Array.from({ length: 30 }, (_, index) => {
			const key = `window-cache-concurrent-${index}`;
			return ensureCachedWindow({
				...cacheOptions(key, async () => {
					startedFetches += 1;
					await fetchGate;
					return [{ bucketStart: index }];
				}),
				requestedRange: { start: 0, end: 100 }
			});
		});

		await Promise.resolve();
		await Promise.resolve();
		try {
			expect(startedFetches).toBeLessThanOrEqual(24);
		} finally {
			releaseFetches();
		}
		await Promise.all(requests);
		expect(startedFetches).toBe(30);

		const refetchEvictedWindow = vi.fn(async () => [{ bucketStart: 0 }]);
		await ensureCachedWindow({
			...cacheOptions('window-cache-concurrent-0', refetchEvictedWindow),
			requestedRange: { start: 0, end: 100 }
		});
		expect(refetchEvictedWindow).toHaveBeenCalledOnce();
	});

	it('enforces the fill limit while distinct ranges for one entry are in flight', async () => {
		let releaseFetches = () => {};
		const fetchGate = new Promise<void>((resolve) => {
			releaseFetches = resolve;
		});
		const key = 'window-cache-concurrent-fills';
		const fetchRange = vi.fn(async (range: TimeRange) => {
			await fetchGate;
			return [{ bucketStart: range.start }];
		});
		const requests = Array.from({ length: 9 }, (_, index) =>
			ensureCachedWindow({
				...cacheOptions(key, fetchRange),
				requestedRange: { start: index * 10, end: (index + 1) * 10 }
			})
		);

		await Promise.resolve();
		await Promise.resolve();
		try {
			expect(fetchRange.mock.calls.length).toBeLessThanOrEqual(8);
		} finally {
			releaseFetches();
		}
		await Promise.all(requests);

		expect(fetchRange).toHaveBeenCalledTimes(9);
		expect(fetchRange).toHaveBeenLastCalledWith({ start: 80, end: 90 }, undefined);
		expect(getMissingWindowRanges(key, { start: 0, end: 80 })).toEqual([{ start: 0, end: 80 }]);
		expect(
			readCachedWindow<Record>(
				key,
				{ start: 80, end: 90 },
				(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
			)
		).toEqual([{ bucketStart: 80 }]);
	});

	it('keeps the previous windows when a rollover fetch fails', async () => {
		const key = 'window-cache-rollover-failure';
		const fetchRange = vi.fn(async (range: TimeRange) => {
			if (range.start === 80) throw new Error('rollover failed');
			return [{ bucketStart: range.start }];
		});
		await fillEightWindows(key, fetchRange);

		await expect(
			ensureCachedWindow({
				...cacheOptions(key, fetchRange),
				requestedRange: { start: 80, end: 90 }
			})
		).rejects.toThrow('rollover failed');

		expect(getMissingWindowRanges(key, { start: 0, end: 80 })).toEqual([]);
		expect(getMissingWindowRanges(key, { start: 80, end: 90 })).toEqual([{ start: 80, end: 90 }]);
		expect(
			readCachedWindow<Record>(
				key,
				{ start: 0, end: 80 },
				(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
			)
		).toEqual(Array.from({ length: 8 }, (_, index) => ({ bucketStart: index * 10 })));
	});

	it('keeps the previous windows when a rollover fetch is aborted', async () => {
		const key = 'window-cache-rollover-abort';
		let releaseRollover = () => {};
		const rolloverGate = new Promise<void>((resolve) => {
			releaseRollover = resolve;
		});
		let rolloverStarted = () => {};
		const started = new Promise<void>((resolve) => {
			rolloverStarted = resolve;
		});
		const fetchRange = vi.fn(async (range: TimeRange) => {
			if (range.start === 80) {
				rolloverStarted();
				await rolloverGate;
			}
			return [{ bucketStart: range.start }];
		});
		await fillEightWindows(key, fetchRange);

		const controller = new AbortController();
		const request = ensureCachedWindow({
			...cacheOptions(key, fetchRange),
			requestedRange: { start: 80, end: 90 },
			signal: controller.signal
		});
		await started;
		controller.abort();
		releaseRollover();

		await expect(request).rejects.toMatchObject({ name: 'AbortError' });
		expect(getMissingWindowRanges(key, { start: 0, end: 80 })).toEqual([]);
		expect(getMissingWindowRanges(key, { start: 80, end: 90 })).toEqual([{ start: 80, end: 90 }]);
	});

	it('does not commit a rollover result after a cache reset', async () => {
		vi.useFakeTimers();
		try {
			vi.setSystemTime(0);
			const key = 'window-cache-rollover-reset';
			let releaseRollover = () => {};
			const rolloverGate = new Promise<void>((resolve) => {
				releaseRollover = resolve;
			});
			let rolloverStarted = () => {};
			const started = new Promise<void>((resolve) => {
				rolloverStarted = resolve;
			});
			let rolloverAttempts = 0;
			const fetchRange = vi.fn(async (range: TimeRange) => {
				if (range.start === 80) {
					rolloverAttempts += 1;
					if (rolloverAttempts === 1) {
						rolloverStarted();
						await rolloverGate;
						return [{ bucketStart: 800 }];
					}
				}
				return [{ bucketStart: range.start }];
			});
			await fillEightWindows(key, fetchRange);
			vi.setSystemTime(1);

			const request = ensureCachedWindow({
				...cacheOptions(key, fetchRange),
				requestedRange: { start: 80, end: 90 },
				maxAgeMs: 10_000
			});
			await started;
			expect(getMissingWindowRanges(key, { start: 0, end: 80 }, 0)).toEqual([
				{ start: 0, end: 80 }
			]);
			releaseRollover();
			await request;

			expect(rolloverAttempts).toBe(2);
			expect(
				readCachedWindow<Record>(
					key,
					{ start: 80, end: 90 },
					(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
				)
			).toEqual([{ bucketStart: 80 }]);
		} finally {
			vi.useRealTimers();
		}
	});

	it('replaces accumulated edges with one complete active window', async () => {
		const key = 'window-cache-retained-windows';
		const fetchRange = vi.fn(async (range: TimeRange) =>
			Array.from({ length: (range.end - range.start) / 10 }, (_, index) => ({
				bucketStart: range.start + index * 10
			}))
		);

		for (let end = 10; end <= 90; end += 10) {
			await ensureCachedWindow({
				...cacheOptions(key, fetchRange),
				requestedRange: { start: 0, end }
			});
		}

		expect(fetchRange).toHaveBeenLastCalledWith({ start: 0, end: 90 }, undefined);
		expect(getMissingWindowRanges(key, { start: 0, end: 90 })).toEqual([]);
		expect(
			readCachedWindow<Record>(
				key,
				{ start: 0, end: 90 },
				(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
			)
		).toEqual(Array.from({ length: 9 }, (_, index) => ({ bucketStart: index * 10 })));
	});

	it('fetches only uncovered edges when a cached window expands', async () => {
		const key = 'window-cache-expansion';
		const fetchRange = vi.fn(async (range: TimeRange) => [{ bucketStart: range.start }]);

		await ensureCachedWindow({
			...cacheOptions(key, fetchRange),
			requestedRange: { start: 100, end: 200 }
		});
		await ensureCachedWindow({
			...cacheOptions(key, fetchRange),
			requestedRange: { start: 50, end: 250 }
		});
		await ensureCachedWindow({
			...cacheOptions(key, fetchRange),
			requestedRange: { start: 100, end: 200 }
		});

		expect(fetchRange.mock.calls.map(([range]) => range)).toEqual([
			{ start: 100, end: 200 },
			{ start: 50, end: 100 },
			{ start: 200, end: 250 }
		]);
		expect(getMissingWindowRanges(key, { start: 50, end: 250 })).toEqual([]);
		expect(
			readCachedWindow<Record>(
				key,
				{ start: 50, end: 250 },
				(record, range) => record.bucketStart >= range.start && record.bucketStart < range.end
			)
		).toEqual([{ bucketStart: 50 }, { bucketStart: 100 }, { bucketStart: 200 }]);
	});

	it('starts a fresh request instead of reusing an aborted in-flight fetch', async () => {
		const key = 'window-cache-abort-replacement';
		const firstController = new AbortController();
		const secondController = new AbortController();
		const firstFetch = vi.fn(
			async () =>
				await new Promise<Record[]>((_, reject) => {
					firstController.signal.addEventListener(
						'abort',
						() => reject(new DOMException('Aborted', 'AbortError')),
						{ once: true }
					);
				})
		);
		const secondFetch = vi.fn(async () => [{ bucketStart: 100 }]);

		const firstRequest = ensureCachedWindow({
			...cacheOptions(key, firstFetch),
			requestedRange: { start: 100, end: 200 },
			signal: firstController.signal
		});
		const firstOutcome = firstRequest.catch((error: unknown) => error);
		firstController.abort();

		const secondRequest = ensureCachedWindow({
			...cacheOptions(key, secondFetch),
			requestedRange: { start: 100, end: 200 },
			signal: secondController.signal
		});

		expect(await firstOutcome).toMatchObject({ name: 'AbortError' });
		await expect(secondRequest).resolves.toBeUndefined();
		expect(firstFetch).toHaveBeenCalledOnce();
		expect(secondFetch).toHaveBeenCalledOnce();
		expect(getMissingWindowRanges(key, { start: 100, end: 200 })).toEqual([]);
	});

	it('removes an aborted same-entry waiter without blocking later requests', async () => {
		let releaseFirstFetch = () => {};
		const firstFetchGate = new Promise<void>((resolve) => {
			releaseFirstFetch = resolve;
		});
		const key = 'window-cache-aborted-waiter';
		const firstRequest = ensureCachedWindow({
			...cacheOptions(key, async () => {
				await firstFetchGate;
				return [{ bucketStart: 0 }];
			}),
			requestedRange: { start: 0, end: 10 }
		});
		const waitingController = new AbortController();
		const waitingFetch = vi.fn(async () => [{ bucketStart: 10 }]);
		const waitingRequest = ensureCachedWindow({
			...cacheOptions(key, waitingFetch),
			requestedRange: { start: 10, end: 20 },
			signal: waitingController.signal
		});

		waitingController.abort();
		await expect(waitingRequest).rejects.toMatchObject({ name: 'AbortError' });
		expect(waitingFetch).not.toHaveBeenCalled();
		releaseFirstFetch();
		await firstRequest;

		const laterFetch = vi.fn(async () => [{ bucketStart: 10 }]);
		await ensureCachedWindow({
			...cacheOptions(key, laterFetch),
			requestedRange: { start: 10, end: 20 }
		});
		expect(laterFetch).toHaveBeenCalledOnce();
	});
});
