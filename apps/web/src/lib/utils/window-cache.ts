export interface TimeRange {
	start: number;
	end: number;
}

interface CacheEntry<T> {
	records: T[];
	segments: TimeRange[];
	oldestCachedAt: number | null;
	fillCount: number;
	generation: number;
	activeUsers: number;
	busy: boolean;
	operationWaiters: Set<() => void>;
}

const cache = new Map<string, CacheEntry<unknown>>();
const slotWaiters = new Set<() => void>();
const MAX_CACHE_ENTRIES = 24;
const MAX_FILLS_PER_ENTRY = 8;

/** Completed windows are reused briefly so navigation stays fast without hiding feed updates. */
export const DEFAULT_WINDOW_CACHE_MAX_AGE_MS = 30_000;

function createEntry<T>(): CacheEntry<T> {
	return {
		records: [],
		segments: [],
		oldestCachedAt: null,
		fillCount: 0,
		generation: 0,
		activeUsers: 0,
		busy: false,
		operationWaiters: new Set()
	};
}

function getExistingEntry<T>(key: string): CacheEntry<T> | undefined {
	const existing = cache.get(key) as CacheEntry<T> | undefined;
	if (!existing) return undefined;
	cache.delete(key);
	cache.set(key, existing as CacheEntry<unknown>);
	return existing;
}

function notifySlotWaiters() {
	for (const resolve of slotWaiters) resolve();
	slotWaiters.clear();
}

function abortReason(signal: AbortSignal): unknown {
	return signal.reason ?? new DOMException('Aborted', 'AbortError');
}

function throwIfAborted(signal?: AbortSignal) {
	if (signal?.aborted) throw abortReason(signal);
}

async function waitForCacheSlot(signal?: AbortSignal): Promise<void> {
	throwIfAborted(signal);
	await new Promise<void>((resolve, reject) => {
		const finish = () => {
			signal?.removeEventListener('abort', handleAbort);
			slotWaiters.delete(finish);
			resolve();
		};
		const handleAbort = () => {
			slotWaiters.delete(finish);
			reject(signal ? abortReason(signal) : new DOMException('Aborted', 'AbortError'));
		};
		slotWaiters.add(finish);
		signal?.addEventListener('abort', handleAbort, { once: true });
	});
}

function notifyNextOperationWaiter(entry: CacheEntry<unknown>) {
	entry.operationWaiters.values().next().value?.();
}

async function waitForEntry(entry: CacheEntry<unknown>, signal?: AbortSignal): Promise<void> {
	throwIfAborted(signal);
	await new Promise<void>((resolve, reject) => {
		const finish = () => {
			signal?.removeEventListener('abort', handleAbort);
			entry.operationWaiters.delete(finish);
			resolve();
		};
		const handleAbort = () => {
			entry.operationWaiters.delete(finish);
			reject(signal ? abortReason(signal) : new DOMException('Aborted', 'AbortError'));
		};
		entry.operationWaiters.add(finish);
		signal?.addEventListener('abort', handleAbort, { once: true });
	});
}

function tryAcquireEntry<T>(key: string): CacheEntry<T> | undefined {
	const existing = getExistingEntry<T>(key);
	if (existing) {
		existing.activeUsers += 1;
		return existing;
	}

	if (cache.size >= MAX_CACHE_ENTRIES) {
		const oldestIdleKey = [...cache].find(([, entry]) => entry.activeUsers === 0)?.[0];
		if (oldestIdleKey === undefined) return undefined;
		cache.delete(oldestIdleKey);
	}

	const created = createEntry<T>();
	created.activeUsers = 1;
	cache.set(key, created as CacheEntry<unknown>);
	return created;
}

function releaseEntry(entry: CacheEntry<unknown>) {
	entry.activeUsers -= 1;
	if (entry.activeUsers === 0) notifySlotWaiters();
}

function normalizeRange(range: TimeRange): TimeRange {
	return {
		start: Math.min(range.start, range.end),
		end: Math.max(range.start, range.end)
	};
}

function mergeSegments(segments: TimeRange[]): TimeRange[] {
	if (segments.length === 0) return [];

	const sorted = segments
		.map(normalizeRange)
		.filter((segment) => segment.start < segment.end)
		.sort((a, b) => a.start - b.start);
	if (sorted.length === 0) return [];

	const merged: TimeRange[] = [{ ...sorted[0] }];
	for (let index = 1; index < sorted.length; index += 1) {
		const segment = sorted[index];
		const last = merged[merged.length - 1];
		if (segment.start <= last.end) {
			last.end = Math.max(last.end, segment.end);
			continue;
		}
		merged.push({ ...segment });
	}

	return merged;
}

function findMissingRanges(entry: CacheEntry<unknown>, requestedRange: TimeRange): TimeRange[] {
	const request = normalizeRange(requestedRange);
	if (request.start === request.end) return [];

	const covered = mergeSegments(entry.segments);
	if (covered.length === 0) return [request];

	const missing: TimeRange[] = [];
	let cursor = request.start;
	for (const segment of covered) {
		if (segment.end <= cursor) continue;
		if (segment.start >= request.end) break;
		if (segment.start > cursor) {
			missing.push({ start: cursor, end: Math.min(segment.start, request.end) });
		}
		cursor = Math.max(cursor, segment.end);
		if (cursor >= request.end) break;
	}

	if (cursor < request.end) missing.push({ start: cursor, end: request.end });
	return missing.filter((segment) => segment.start < segment.end);
}

function resolveMaxAge(maxAgeMs: number | undefined): number {
	const resolved = maxAgeMs ?? DEFAULT_WINDOW_CACHE_MAX_AGE_MS;
	if (Number.isNaN(resolved) || resolved < 0) {
		throw new RangeError('Window cache maxAgeMs must be non-negative');
	}
	return resolved;
}

function clearEntry(entry: CacheEntry<unknown>) {
	entry.records = [];
	entry.segments = [];
	entry.oldestCachedAt = null;
	entry.fillCount = 0;
	entry.generation += 1;
}

function expireEntry(entry: CacheEntry<unknown>, maxAgeMs: number) {
	if (entry.oldestCachedAt !== null && Date.now() - entry.oldestCachedAt >= maxAgeMs) {
		clearEntry(entry);
	}
}

export function getMissingWindowRanges(
	key: string,
	requestedRange: TimeRange,
	maxAgeMs?: number
): TimeRange[] {
	const entry = getExistingEntry(key);
	if (!entry) {
		const request = normalizeRange(requestedRange);
		return request.start === request.end ? [] : [request];
	}
	expireEntry(entry, resolveMaxAge(maxAgeMs));
	return findMissingRanges(entry, requestedRange);
}

export function readCachedWindow<T>(
	key: string,
	requestedRange: TimeRange,
	matchesRange: (record: T, requestedRange: TimeRange) => boolean,
	maxAgeMs?: number
): T[] {
	const entry = getExistingEntry<T>(key);
	if (!entry) return [];
	expireEntry(entry, resolveMaxAge(maxAgeMs));
	return entry.records.filter((record) => matchesRange(record, requestedRange));
}

export async function ensureCachedWindow<T>(options: {
	key: string;
	requestedRange: TimeRange;
	fetchRange: (range: TimeRange, signal?: AbortSignal) => Promise<T[]>;
	getRecordKey: (record: T) => string;
	compareRecords: (left: T, right: T) => number;
	signal?: AbortSignal;
	maxAgeMs?: number;
}): Promise<void> {
	throwIfAborted(options.signal);
	const entry = tryAcquireEntry<T>(options.key);
	if (!entry) {
		await waitForCacheSlot(options.signal);
		return await ensureCachedWindow(options);
	}
	if (entry.busy) {
		try {
			await waitForEntry(entry as CacheEntry<unknown>, options.signal);
		} finally {
			releaseEntry(entry as CacheEntry<unknown>);
		}
		if (options.signal?.aborted) {
			if (!entry.busy) notifyNextOperationWaiter(entry as CacheEntry<unknown>);
			throw abortReason(options.signal);
		}
		return await ensureCachedWindow(options);
	}
	entry.busy = true;
	let retry = false;
	try {
		const maxAgeMs = resolveMaxAge(options.maxAgeMs);
		expireEntry(entry, maxAgeMs);
		const missingRanges = findMissingRanges(entry, options.requestedRange);
		if (missingRanges.length === 0) return;

		const replacingWindow = entry.fillCount + missingRanges.length > MAX_FILLS_PER_ENTRY;
		const fetchRanges = replacingWindow ? [normalizeRange(options.requestedRange)] : missingRanges;
		const generation = entry.generation;

		const fetchedRanges = await Promise.all(
			fetchRanges.map((range) => options.fetchRange(range, options.signal))
		);
		throwIfAborted(options.signal);

		if (entry.generation !== generation) {
			retry = true;
		} else if (replacingWindow) {
			const replacement = new Map<string, T>();
			for (const record of fetchedRanges.flat()) {
				replacement.set(options.getRecordKey(record), record);
			}

			entry.records = [...replacement.values()].sort(options.compareRecords);
			entry.segments = mergeSegments(fetchRanges);
			entry.fillCount = fetchRanges.length;
			entry.oldestCachedAt = Date.now();
		} else {
			const merged = new Map(entry.records.map((record) => [options.getRecordKey(record), record]));
			for (const record of fetchedRanges.flat()) {
				merged.set(options.getRecordKey(record), record);
			}

			entry.records = [...merged.values()].sort(options.compareRecords);
			entry.segments = mergeSegments([...entry.segments, ...missingRanges]);
			entry.fillCount += missingRanges.length;
			entry.oldestCachedAt ??= Date.now();
		}
	} finally {
		entry.busy = false;
		notifyNextOperationWaiter(entry as CacheEntry<unknown>);
		releaseEntry(entry as CacheEntry<unknown>);
	}
	if (retry) return await ensureCachedWindow(options);
}
