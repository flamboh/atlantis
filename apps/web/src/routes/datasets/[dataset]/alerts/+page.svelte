<script lang="ts">
	import { afterNavigate } from '$app/navigation';
	import { untrack } from 'svelte';
	import type { PageProps } from './$types';
	import DatasetTabs from '$lib/components/datasets/DatasetTabs.svelte';
	import type { AlertHorizon, AlertsFeedResponse, AlertSort, AlertTail } from '$lib/types/types';

	type TailSelection = AlertTail;
	type ErrorResponse = { data: null; error: string };

	const PAGE_SIZE = 100;
	const MAX_LIMIT = 500;
	const REFRESH_INTERVAL_MS = 30_000;
	const LIVE_WINDOW_AGE_MS = 15 * 60_000;
	const NEW_ADDRESS_AGE_SECONDS = 15 * 60;
	const CONTROL_GROUP_CLASS =
		'dark:border-dark-border dark:bg-dark-subtle grid w-full gap-0.5 rounded-md border border-gray-200 bg-gray-50 p-1 sm:w-fit';
	const CONTROL_BUTTON_CLASS =
		'flex min-h-7 items-center justify-center rounded px-2.5 py-0.5 text-center text-xs font-medium transition-colors focus:ring-2 focus:ring-blue-500 focus:outline-none';
	const CONTROL_BUTTON_INACTIVE_CLASS =
		'text-gray-700 hover:text-gray-900 dark:text-gray-300 dark:hover:text-gray-100';
	const CONTROL_BUTTON_ACTIVE_CLASS = 'bg-blue-600 text-white shadow-sm';
	const TAIL_OPTIONS: Array<{ value: TailSelection; label: string; title: string }> = [
		{
			value: 'high',
			label: 'High α',
			title: 'Isolated addresses in sparse regions of address space'
		},
		{ value: 'low', label: 'Low α', title: 'Addresses deep inside dense clusters' }
	];
	const HORIZON_OPTIONS: Array<{ value: AlertHorizon; label: string }> = [
		{ value: '1h', label: '1h' },
		{ value: '6h', label: '6h' },
		{ value: '24h', label: '24h' },
		{ value: '7d', label: '7d' }
	];
	const SORT_OPTIONS: Array<{ value: AlertSort; label: string }> = [
		{ value: 'extreme', label: 'Most extreme' },
		{ value: 'recent', label: 'Recent' }
	];
	const timeFormatter = new Intl.DateTimeFormat(undefined, {
		hour: '2-digit',
		minute: '2-digit'
	});
	const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short'
	});
	const countFormatter = new Intl.NumberFormat();

	let { data }: PageProps = $props();
	let activeDataset = $state(untrack(() => data.selectedDataset));
	let feedResponse = $state<AlertsFeedResponse>(untrack(() => data.alerts));
	let selectedTail = $state<TailSelection>('high');
	let selectedHorizon = $state<AlertHorizon>('24h');
	let selectedSort = $state<AlertSort>('extreme');
	let limit = $state(PAGE_SIZE);
	let now = $state(Date.now());
	let loading = $state(false);
	let loadingMore = $state(false);
	let fetchError = $state('');
	let copied = $state(false);
	let copyError = $state('');
	let requestGeneration = 0;

	const selectedDatasetLabel = $derived(
		data.datasets.find((dataset) => dataset.datasetId === data.selectedDataset)?.label ??
			data.selectedDataset
	);
	const feedCommand = $derived(`netflow-db feed ${data.selectedDataset}`);
	const canShowMore = $derived(
		feedResponse.addresses.length < feedResponse.totalAddresses && limit < MAX_LIMIT
	);
	const statusText = $derived.by(() => {
		if (!feedResponse.feed.present) {
			return 'Feed not running';
		}
		if (
			feedResponse.feed.latestWindowStart === null ||
			feedResponse.feed.latestWindowEnd === null ||
			feedResponse.feed.latestAddressCount === null
		) {
			return 'Feed idle · no windows processed';
		}

		const windowStart = feedResponse.feed.latestWindowStart;
		const windowEnd = feedResponse.feed.latestWindowEnd;
		const windowAge = now - windowEnd * 1000;
		if (windowAge >= 0 && windowAge < LIVE_WINDOW_AGE_MS) {
			return `Live · last window ${formatTime(windowStart)}–${formatTime(windowEnd)} · ${countFormatter.format(feedResponse.feed.latestAddressCount)} addresses scored`;
		}

		return `Feed idle · last window ${dateTimeFormatter.format(new Date(windowEnd * 1000))}`;
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

	// "New" = the address's first appearance in the entire retained history
	// (firstSeen is retention-wide, not horizon-scoped) is within the last few
	// windows — it just entered the anomalous set for the first time.
	function isNewAddress(firstSeen: number): boolean {
		return (
			feedResponse.feed.present &&
			feedResponse.feed.latestWindowStart !== null &&
			firstSeen >= feedResponse.feed.latestWindowStart - NEW_ADDRESS_AGE_SECONDS
		);
	}

	function feedUrl(
		tail: TailSelection,
		horizon: AlertHorizon,
		sort: AlertSort,
		requestLimit: number
	): string {
		const params = [
			`dataset=${encodeURIComponent(data.selectedDataset)}`,
			`tail=${tail}`,
			`horizon=${horizon}`,
			`sort=${sort}`,
			`limit=${requestLimit}`
		];
		return `/api/alerts?${params.join('&')}`;
	}

	async function requestFeed(
		tail: TailSelection,
		horizon: AlertHorizon,
		sort: AlertSort,
		requestLimit: number
	): Promise<AlertsFeedResponse> {
		const response = await fetch(feedUrl(tail, horizon, sort, requestLimit));
		const payload = (await response.json()) as AlertsFeedResponse | ErrorResponse;
		if (!response.ok || 'error' in payload) {
			throw new Error('error' in payload ? payload.error : 'Failed to load alerts feed');
		}
		return payload;
	}

	async function loadFeed(
		tail: TailSelection,
		horizon: AlertHorizon,
		sort: AlertSort,
		requestLimit: number,
		showLoading: boolean
	): Promise<boolean> {
		const generation = ++requestGeneration;
		if (showLoading) {
			loading = true;
		}
		try {
			const nextResponse = await requestFeed(tail, horizon, sort, requestLimit);
			if (generation !== requestGeneration) {
				return false;
			}

			feedResponse = nextResponse;
			fetchError = '';
			return true;
		} catch (error) {
			if (generation === requestGeneration) {
				fetchError = error instanceof Error ? error.message : 'Failed to load alerts feed';
			}
			return false;
		} finally {
			if (generation === requestGeneration && showLoading) {
				loading = false;
			}
		}
	}

	async function selectTail(tail: TailSelection): Promise<void> {
		if (tail === selectedTail) {
			return;
		}

		selectedTail = tail;
		limit = PAGE_SIZE;
		await loadFeed(tail, selectedHorizon, selectedSort, limit, true);
	}

	async function selectHorizon(horizon: AlertHorizon): Promise<void> {
		if (horizon === selectedHorizon) {
			return;
		}

		selectedHorizon = horizon;
		limit = PAGE_SIZE;
		await loadFeed(selectedTail, horizon, selectedSort, limit, true);
	}

	async function selectSort(sort: AlertSort): Promise<void> {
		if (sort === selectedSort) {
			return;
		}

		selectedSort = sort;
		limit = PAGE_SIZE;
		await loadFeed(selectedTail, selectedHorizon, sort, limit, true);
	}

	async function refreshFeed(): Promise<void> {
		if (loading || loadingMore) {
			return;
		}
		await loadFeed(selectedTail, selectedHorizon, selectedSort, limit, false);
	}

	async function showMore(): Promise<void> {
		if (!canShowMore || loading || loadingMore) {
			return;
		}

		const nextLimit = Math.min(MAX_LIMIT, limit + PAGE_SIZE);
		loadingMore = true;
		try {
			if (await loadFeed(selectedTail, selectedHorizon, selectedSort, nextLimit, false)) {
				limit = nextLimit;
			}
		} finally {
			loadingMore = false;
		}
	}

	async function copyCommand(): Promise<void> {
		try {
			await navigator.clipboard.writeText(feedCommand);
			copied = true;
			copyError = '';
		} catch {
			copied = false;
			copyError = 'Could not copy the command';
		}
	}

	afterNavigate(() => {
		if (activeDataset === data.selectedDataset) {
			return;
		}

		requestGeneration += 1;
		activeDataset = data.selectedDataset;
		feedResponse = data.alerts;
		selectedTail = 'high';
		selectedHorizon = '24h';
		selectedSort = 'extreme';
		limit = PAGE_SIZE;
		now = Date.now();
		loading = false;
		loadingMore = false;
		fetchError = '';
		copied = false;
		copyError = '';
	});

	// The initial page comes from +page.ts. This one effect only owns polling and tab visibility.
	$effect(() => {
		let interval: ReturnType<typeof setInterval> | undefined;
		const stopTimer = () => {
			if (interval !== undefined) {
				clearInterval(interval);
				interval = undefined;
			}
		};
		const startTimer = () => {
			stopTimer();
			if (document.visibilityState === 'visible') {
				interval = setInterval(() => {
					now = Date.now();
					void refreshFeed();
				}, REFRESH_INTERVAL_MS);
			}
		};
		const handleVisibilityChange = () => {
			if (document.visibilityState === 'visible') {
				now = Date.now();
				void refreshFeed();
			}
			startTimer();
		};

		document.addEventListener('visibilitychange', handleVisibilityChange);
		startTimer();

		return () => {
			stopTimer();
			document.removeEventListener('visibilitychange', handleVisibilityChange);
		};
	});
</script>

<svelte:head>
	<title>Singularity alerts | ATLANTIS</title>
	<meta
		name="description"
		content="Continuous Singularity alerts for anomalous network addresses"
	/>
</svelte:head>

<main class="mx-auto flex max-w-[95vw] flex-col gap-4 px-4 py-8 sm:px-2 lg:px-4">
	<header>
		<h1 class="text-2xl font-semibold text-gray-900 dark:text-gray-100">Singularity alerts</h1>
		<p class="mt-1 text-sm text-gray-600 dark:text-gray-300">{selectedDatasetLabel}</p>
		<p class="mt-2 text-sm text-gray-500 dark:text-gray-400" aria-live="polite">
			{statusText}
		</p>
	</header>
	<DatasetTabs datasetId={data.selectedDataset} active="alerts" />

	{#if fetchError}
		<div
			class="rounded-lg border border-red-200 bg-red-50 p-4 text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-400"
			role="alert"
		>
			{fetchError}
		</div>
	{/if}

	{#if !feedResponse.feed.present}
		<section
			class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white p-4 text-gray-600 shadow-sm dark:text-gray-400"
		>
			<h2 class="font-medium text-gray-900 dark:text-gray-100">The alert feed is not running</h2>
			<p class="mt-1 text-sm">Start it for this dataset with:</p>
			<div
				class="dark:bg-dark-subtle mt-3 flex flex-col gap-2 rounded-md bg-gray-100 p-3 sm:flex-row sm:items-center sm:justify-between"
			>
				<code class="overflow-x-auto font-mono text-sm text-gray-900 dark:text-gray-100"
					>{feedCommand}</code
				>
				<button
					type="button"
					onclick={copyCommand}
					class="shrink-0 rounded bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:outline-none"
				>
					{copied ? 'Copied' : 'Copy'}
				</button>
			</div>
			{#if copyError}
				<p class="mt-2 text-sm text-red-600 dark:text-red-400" role="alert">{copyError}</p>
			{/if}
		</section>
	{:else}
		<div class="flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
			<div class="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-end">
				<div class="flex flex-col gap-1">
					<span class="text-xs font-medium text-gray-500 dark:text-gray-400">Alpha</span>
					<div
						class={`${CONTROL_GROUP_CLASS} grid-cols-2`}
						role="group"
						aria-label="Filter alerts by alpha tail"
					>
						{#each TAIL_OPTIONS as option (option.value)}
							<button
								type="button"
								onclick={() => selectTail(option.value)}
								title={option.title}
								class={`${CONTROL_BUTTON_CLASS} ${
									selectedTail === option.value
										? CONTROL_BUTTON_ACTIVE_CLASS
										: CONTROL_BUTTON_INACTIVE_CLASS
								}`}
								aria-pressed={selectedTail === option.value}
							>
								{option.label}
							</button>
						{/each}
					</div>
				</div>

				<div class="flex flex-col gap-1">
					<span class="text-xs font-medium text-gray-500 dark:text-gray-400">Horizon</span>
					<div
						class={`${CONTROL_GROUP_CLASS} grid-cols-4`}
						role="group"
						aria-label="Select alert horizon"
					>
						{#each HORIZON_OPTIONS as option (option.value)}
							<button
								type="button"
								onclick={() => selectHorizon(option.value)}
								class={`${CONTROL_BUTTON_CLASS} ${
									selectedHorizon === option.value
										? CONTROL_BUTTON_ACTIVE_CLASS
										: CONTROL_BUTTON_INACTIVE_CLASS
								}`}
								aria-pressed={selectedHorizon === option.value}
							>
								{option.label}
							</button>
						{/each}
					</div>
				</div>

				<div class="flex flex-col gap-1">
					<span class="text-xs font-medium text-gray-500 dark:text-gray-400">Sort</span>
					<div
						class={`${CONTROL_GROUP_CLASS} grid-cols-2`}
						role="group"
						aria-label="Sort alert addresses"
					>
						{#each SORT_OPTIONS as option (option.value)}
							<button
								type="button"
								onclick={() => selectSort(option.value)}
								class={`${CONTROL_BUTTON_CLASS} ${
									selectedSort === option.value
										? CONTROL_BUTTON_ACTIVE_CLASS
										: CONTROL_BUTTON_INACTIVE_CLASS
								}`}
								aria-pressed={selectedSort === option.value}
							>
								{option.label}
							</button>
						{/each}
					</div>
				</div>

				{#if feedResponse.feed.present && feedResponse.feed.thresholds}
					<div class="flex flex-col gap-1">
						<span class="text-xs font-medium text-gray-500 dark:text-gray-400">Thresholds</span>
						<p
							class="flex min-h-7 items-center pb-1 text-sm text-gray-500 tabular-nums dark:text-gray-400"
							title="The feed records an address when its alpha crosses either calibrated bound"
						>
							α ≥ {feedResponse.feed.thresholds.high} · α ≤ {feedResponse.feed.thresholds.low}
						</p>
					</div>
				{/if}
			</div>
		</div>

		{#if feedResponse.addresses.length === 0}
			<div
				class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white px-4 py-3 text-sm text-gray-500 shadow-sm dark:text-gray-400"
			>
				No anomalous addresses in the last {selectedHorizon}.
			</div>
		{:else}
			<section
				class={`dark:border-dark-border dark:bg-dark-surface overflow-x-auto rounded-lg border bg-white shadow-sm transition-opacity ${loading ? 'opacity-60' : ''}`}
				aria-label="Anomalous addresses"
				aria-busy={loading}
			>
				<table class="w-full text-sm">
					<thead>
						<tr
							class="dark:border-dark-border border-b text-xs font-medium text-gray-500 dark:text-gray-400"
						>
							<th class="w-full px-4 py-2 text-left font-medium">Address</th>
							<th
								class={`px-4 py-2 text-right font-medium whitespace-nowrap ${
									selectedSort === 'extreme' ? 'text-gray-900 dark:text-gray-100' : ''
								}`}
							>
								Peak α
							</th>
							<th
								class={`px-4 py-2 text-right font-medium whitespace-nowrap ${
									selectedSort === 'recent' ? 'text-gray-900 dark:text-gray-100' : ''
								}`}
							>
								Latest α
							</th>
							<th class="px-4 py-2 text-right font-medium">First seen</th>
							<th class="px-4 py-2 text-right font-medium">Last seen</th>
							<th class="px-4 py-2 text-right font-medium" title="Windows flagged in this horizon">
								Flagged
							</th>
							<th class="px-4 py-2 text-right font-medium">r²</th>
						</tr>
					</thead>
					<tbody class="dark:divide-dark-border divide-y divide-gray-100">
						{#each feedResponse.addresses as alert (alert.address)}
							<tr>
								<td class="px-4 py-3">
									<div class="flex min-w-0 items-center gap-2">
										<span class="truncate font-mono text-gray-900 dark:text-gray-100">
											{alert.address}
										</span>
										{#if isNewAddress(alert.firstSeen)}
											<span
												title={`First flagged ${formatRelativeTime(alert.firstSeen)} — never seen in the retained history before that`}
												class="shrink-0 rounded-full bg-amber-100 px-2 py-0.5 text-xs font-medium text-amber-700 dark:bg-amber-950 dark:text-amber-300"
											>
												new
											</span>
										{/if}
									</div>
								</td>
								<td
									class={`px-4 py-3 text-right tabular-nums ${
										selectedSort === 'extreme'
											? 'font-medium text-gray-900 dark:text-gray-100'
											: 'text-gray-400 dark:text-gray-500'
									}`}
								>
									{alert.peakAlpha.toFixed(3)}
								</td>
								<td
									class={`px-4 py-3 text-right tabular-nums ${
										selectedSort === 'recent'
											? 'font-medium text-gray-900 dark:text-gray-100'
											: 'text-gray-400 dark:text-gray-500'
									}`}
								>
									{alert.latestAlpha.toFixed(3)}
								</td>
								<td
									title={dateTimeFormatter.format(new Date(alert.firstSeen * 1000))}
									class="px-4 py-3 text-right whitespace-nowrap text-gray-500 tabular-nums dark:text-gray-400"
								>
									{formatRelativeTime(alert.firstSeen)}
								</td>
								<td
									title={dateTimeFormatter.format(new Date(alert.lastSeen * 1000))}
									class="px-4 py-3 text-right whitespace-nowrap text-gray-500 tabular-nums dark:text-gray-400"
								>
									{formatRelativeTime(alert.lastSeen)}
								</td>
								<td
									title={`flagged in ${countFormatter.format(alert.timesFlagged)} windows in this horizon`}
									class="px-4 py-3 text-right text-gray-600 tabular-nums dark:text-gray-300"
								>
									{countFormatter.format(alert.timesFlagged)}×
								</td>
								<td class="px-4 py-3 text-right text-gray-500 tabular-nums dark:text-gray-400">
									{alert.peakR2.toFixed(2)}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</section>
		{/if}

		<div class="flex flex-col items-center gap-2 pt-1">
			{#if canShowMore}
				<button
					type="button"
					onclick={showMore}
					disabled={loadingMore}
					class="dark:border-dark-border dark:bg-dark-surface rounded-md border border-gray-300 bg-white px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 focus:ring-2 focus:ring-blue-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60 dark:text-gray-300 dark:hover:bg-gray-800"
				>
					{loadingMore ? 'Loading…' : 'Show more'}
				</button>
			{/if}
			<p class="text-xs text-gray-500 dark:text-gray-400">
				showing {countFormatter.format(feedResponse.addresses.length)} of {countFormatter.format(
					feedResponse.totalAddresses
				)}
			</p>
		</div>
	{/if}
</main>
