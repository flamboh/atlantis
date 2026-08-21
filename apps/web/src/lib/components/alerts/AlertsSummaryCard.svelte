<script lang="ts">
	import { resolve } from '$app/paths';
	import type { AlertsFeedResponse } from '$lib/types/types';
	import { untrack } from 'svelte';

	type ErrorResponse = { data: null; error: string };

	const LIVE_WINDOW_AGE_MS = 15 * 60_000;
	const NEW_ADDRESS_AGE_SECONDS = 15 * 60;
	const timeFormatter = new Intl.DateTimeFormat(undefined, {
		hour: '2-digit',
		minute: '2-digit'
	});
	const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short'
	});

	let {
		datasetId,
		initialFeed = null
	}: { datasetId: string; initialFeed?: AlertsFeedResponse | null } = $props();
	let feedResponse = $state<AlertsFeedResponse | null>(
		untrack(() => (initialFeed?.feed.present ? initialFeed : null))
	);
	let now = $state(Date.now());

	const statusText = $derived.by(() => {
		if (!feedResponse?.feed.present) {
			return '';
		}
		if (
			feedResponse.feed.latestWindowStart === null ||
			feedResponse.feed.latestWindowEnd === null
		) {
			return 'Idle · no windows processed';
		}

		const windowAge = now - feedResponse.feed.latestWindowEnd * 1000;
		if (windowAge >= 0 && windowAge < LIVE_WINDOW_AGE_MS) {
			return `Live · last window ${formatTime(feedResponse.feed.latestWindowStart)}–${formatTime(feedResponse.feed.latestWindowEnd)}`;
		}

		return `Idle · ${dateTimeFormatter.format(new Date(feedResponse.feed.latestWindowEnd * 1000))}`;
	});

	function formatTime(timestamp: number): string {
		return timeFormatter.format(new Date(timestamp * 1000));
	}

	function formatRelativeTime(timestamp: number): string {
		const ageMs = Math.max(0, now - timestamp * 1000);
		if (ageMs < 60_000) {
			return 'just now';
		}
		if (ageMs < 60 * 60_000) {
			return `${Math.floor(ageMs / 60_000)} min ago`;
		}
		if (ageMs < 24 * 60 * 60_000) {
			return `${Math.floor(ageMs / (60 * 60_000))} h ago`;
		}
		return dateTimeFormatter.format(new Date(timestamp * 1000));
	}

	function isNewAddress(firstSeen: number): boolean {
		return (
			feedResponse?.feed.present === true &&
			feedResponse.feed.latestWindowStart !== null &&
			firstSeen >= feedResponse.feed.latestWindowStart - NEW_ADDRESS_AGE_SECONDS
		);
	}

	async function loadFeed(requestedDataset: string, signal: AbortSignal): Promise<void> {
		try {
			const response = await fetch(
				`/api/alerts?dataset=${encodeURIComponent(requestedDataset)}&tail=high&horizon=24h&sort=extreme&limit=5`,
				{ signal }
			);
			const payload = (await response.json()) as AlertsFeedResponse | ErrorResponse;
			if (!response.ok || 'error' in payload) {
				throw new Error('error' in payload ? payload.error : 'Failed to load alerts feed');
			}
			if (!signal.aborted) {
				now = Date.now();
				feedResponse = payload.feed.present ? payload : null;
			}
		} catch {
			if (!signal.aborted) {
				feedResponse = null;
			}
		}
	}

	$effect(() => {
		const requestedDataset = datasetId;
		const preloadedFeed = initialFeed;
		if (preloadedFeed !== null) {
			now = Date.now();
			feedResponse = preloadedFeed.feed.present ? preloadedFeed : null;
			return;
		}

		const controller = new AbortController();
		feedResponse = null;
		void loadFeed(requestedDataset, controller.signal);

		return () => {
			controller.abort();
		};
	});
</script>

{#if feedResponse?.feed.present}
	<section
		class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white shadow-sm dark:shadow-none"
		aria-label="Singularity alerts"
	>
		<header
			class="dark:border-dark-border flex flex-col gap-2 border-b px-4 py-3 sm:flex-row sm:items-center sm:justify-between"
		>
			<div>
				<h2 class="font-medium text-gray-900 dark:text-gray-100">Singularity alerts</h2>
				<p class="mt-0.5 text-xs text-gray-500 dark:text-gray-400">{statusText}</p>
			</div>
			<a
				href={resolve('/datasets/[dataset]/alerts', { dataset: datasetId })}
				class="shrink-0 text-sm font-medium text-blue-600 hover:text-blue-700 hover:underline focus:ring-2 focus:ring-blue-500 focus:outline-none dark:text-blue-400 dark:hover:text-blue-300"
			>
				View all <span aria-hidden="true">→</span>
			</a>
		</header>

		{#if feedResponse.addresses.length === 0}
			<p class="px-4 py-3 text-sm text-gray-500 dark:text-gray-400">
				No anomalous addresses in the last 24h.
			</p>
		{:else}
			<div class="dark:divide-dark-border divide-y divide-gray-100">
				{#each feedResponse.addresses.slice(0, 5) as alert (alert.address)}
					<div class="flex items-center justify-between gap-4 px-4 py-2.5 text-sm">
						<div class="flex min-w-0 items-center gap-2">
							<span class="truncate font-mono text-gray-900 dark:text-gray-100">
								{alert.address}
							</span>
							{#if isNewAddress(alert.firstSeen)}
								<span
									class="shrink-0 rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-700 dark:bg-amber-950 dark:text-amber-300"
								>
									new
								</span>
							{/if}
						</div>
						<div class="flex shrink-0 items-center gap-4 text-xs text-gray-500 dark:text-gray-400">
							<span class="tabular-nums">peak α {alert.peakAlpha.toFixed(3)}</span>
							<span class="w-20 text-right whitespace-nowrap tabular-nums">
								{formatRelativeTime(alert.lastSeen)}
							</span>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</section>
{/if}
