<script lang="ts">
	import { afterNavigate } from '$app/navigation';
	import { untrack } from 'svelte';
	import type { PageProps } from './$types';
	import type { AlertsFeedResponse, AlertTail } from '$lib/types/types';

	type TailSelection = 'all' | AlertTail;
	type ErrorResponse = { data: null; error: string };

	const PAGE_SIZE = 24;
	const REFRESH_INTERVAL_MS = 30_000;
	const LIVE_WINDOW_AGE_MS = 15 * 60_000;
	const CONTROL_GROUP_CLASS =
		'dark:border-dark-border dark:bg-dark-subtle grid w-full gap-0.5 rounded-md border border-gray-200 bg-gray-50 p-1 sm:w-fit';
	const CONTROL_BUTTON_CLASS =
		'flex min-h-7 items-center justify-center rounded px-2.5 py-0.5 text-center text-xs font-medium transition-colors focus:ring-2 focus:ring-blue-500 focus:outline-none sm:min-w-20';
	const CONTROL_BUTTON_INACTIVE_CLASS =
		'text-gray-700 hover:text-gray-900 dark:text-gray-300 dark:hover:text-gray-100';
	const CONTROL_BUTTON_ACTIVE_CLASS = 'bg-blue-600 text-white shadow-sm';
	const TAIL_OPTIONS: Array<{ value: TailSelection; label: string }> = [
		{ value: 'all', label: 'All' },
		{ value: 'high', label: 'High α' },
		{ value: 'low', label: 'Low α' }
	];
	const timeFormatter = new Intl.DateTimeFormat(undefined, {
		hour: '2-digit',
		minute: '2-digit'
	});
	const dateFormatter = new Intl.DateTimeFormat(undefined, {
		year: 'numeric',
		month: 'short',
		day: 'numeric'
	});
	const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short'
	});
	const countFormatter = new Intl.NumberFormat();

	let { data }: PageProps = $props();
	let activeDataset = $state(untrack(() => data.selectedDataset));
	let feedResponse = $state<AlertsFeedResponse>(untrack(() => data.alerts));
	let selectedTail = $state<TailSelection>('all');
	let autoRefresh = $state(true);
	let now = $state(Date.now());
	let loading = $state(false);
	let loadingOlder = $state(false);
	let hasOlder = $state(untrack(() => data.alerts.windows.length === PAGE_SIZE));
	let fetchError = $state('');
	let copied = $state(false);
	let copyError = $state('');
	let requestGeneration = 0;

	const selectedDatasetLabel = $derived(
		data.datasets.find((dataset) => dataset.datasetId === data.selectedDataset)?.label ??
			data.selectedDataset
	);
	const newestWindow = $derived(feedResponse.windows[0]);
	const oldestWindow = $derived(feedResponse.windows.at(-1));
	const feedCommand = $derived(`netflow-db feed ${data.selectedDataset || '<dataset-id>'}`);
	const statusText = $derived.by(() => {
		if (!feedResponse.feed.present) {
			return 'Feed not running';
		}
		if (!newestWindow) {
			return 'Feed idle · no windows processed';
		}

		const windowAge = now - newestWindow.windowEnd * 1000;
		if (windowAge >= 0 && windowAge < LIVE_WINDOW_AGE_MS) {
			return `Live · last window ${formatTime(newestWindow.windowStart)}–${formatTime(newestWindow.windowEnd)} · ${countFormatter.format(newestWindow.addressCount)} addresses scored`;
		}

		return `Feed idle · last window ${dateTimeFormatter.format(new Date(newestWindow.windowEnd * 1000))}`;
	});

	function formatTime(timestamp: number): string {
		return timeFormatter.format(new Date(timestamp * 1000));
	}

	function formatDate(timestamp: number): string {
		return dateFormatter.format(new Date(timestamp * 1000));
	}

	function feedUrl(tail: TailSelection, before?: number): string {
		const params = [
			`dataset=${encodeURIComponent(data.selectedDataset)}`,
			`limitWindows=${PAGE_SIZE}`
		];
		if (tail !== 'all') {
			params.push(`tail=${tail}`);
		}
		if (before !== undefined) {
			params.push(`before=${before}`);
		}
		return `/api/alerts?${params.join('&')}`;
	}

	async function requestFeed(tail: TailSelection, before?: number): Promise<AlertsFeedResponse> {
		const response = await fetch(feedUrl(tail, before));
		const payload = (await response.json()) as AlertsFeedResponse | ErrorResponse;
		if (!response.ok || 'error' in payload) {
			throw new Error('error' in payload ? payload.error : 'Failed to load alerts feed');
		}
		return payload;
	}

	function mergeWindows(
		latest: AlertsFeedResponse,
		current: AlertsFeedResponse
	): AlertsFeedResponse {
		if (!latest.feed.present) {
			return latest;
		}

		return {
			feed: latest.feed,
			windows: mergeWindowLists(latest.windows, current.windows)
		};
	}

	function mergeWindowLists(
		preferred: AlertsFeedResponse['windows'],
		additional: AlertsFeedResponse['windows']
	): AlertsFeedResponse['windows'] {
		const windows = [...preferred];
		for (const window of additional) {
			if (!windows.some((candidate) => candidate.windowStart === window.windowStart)) {
				windows.push(window);
			}
		}
		return windows.sort((left, right) => right.windowStart - left.windowStart);
	}

	async function loadFirstPage(tail: TailSelection, preserveOlder: boolean): Promise<void> {
		if (!data.selectedDataset) {
			return;
		}

		const generation = ++requestGeneration;
		if (!preserveOlder) {
			loading = true;
		}
		try {
			const nextResponse = await requestFeed(tail);
			if (generation !== requestGeneration) {
				return;
			}

			feedResponse = preserveOlder ? mergeWindows(nextResponse, feedResponse) : nextResponse;
			if (!preserveOlder) {
				hasOlder = nextResponse.windows.length === PAGE_SIZE;
			}
			fetchError = '';
		} catch (error) {
			if (generation === requestGeneration) {
				fetchError = error instanceof Error ? error.message : 'Failed to load alerts feed';
			}
		} finally {
			if (generation === requestGeneration && !preserveOlder) {
				loading = false;
			}
		}
	}

	async function selectTail(tail: TailSelection): Promise<void> {
		if (tail === selectedTail) {
			return;
		}

		loadingOlder = false;
		selectedTail = tail;
		await loadFirstPage(tail, false);
	}

	async function refreshFeed(): Promise<void> {
		if (loading || loadingOlder) {
			return;
		}
		await loadFirstPage(selectedTail, true);
	}

	async function loadOlderWindows(): Promise<void> {
		if (!oldestWindow || loading || loadingOlder || !hasOlder) {
			return;
		}

		const generation = ++requestGeneration;
		loadingOlder = true;
		try {
			const olderResponse = await requestFeed(selectedTail, oldestWindow.windowStart);
			if (generation !== requestGeneration) {
				return;
			}

			if (!olderResponse.feed.present) {
				feedResponse = olderResponse;
				hasOlder = false;
			} else {
				feedResponse = {
					feed: olderResponse.feed,
					windows: mergeWindowLists(feedResponse.windows, olderResponse.windows)
				};
				hasOlder = olderResponse.windows.length === PAGE_SIZE;
			}
			fetchError = '';
		} catch (error) {
			if (generation === requestGeneration) {
				fetchError = error instanceof Error ? error.message : 'Failed to load older windows';
			}
		} finally {
			if (generation === requestGeneration) {
				loadingOlder = false;
			}
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
		selectedTail = 'all';
		loading = false;
		loadingOlder = false;
		hasOlder = data.alerts.windows.length === PAGE_SIZE;
		fetchError = '';
		copied = false;
		copyError = '';
	});

	// The initial page comes from +page.ts. This one effect only owns polling and tab visibility.
	$effect(() => {
		if (!autoRefresh) {
			return;
		}

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
		<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
			<div
				class={`${CONTROL_GROUP_CLASS} grid-cols-3`}
				role="group"
				aria-label="Filter alerts by tail"
			>
				{#each TAIL_OPTIONS as option (option.value)}
					<button
						type="button"
						onclick={() => selectTail(option.value)}
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

			<label
				class="flex cursor-pointer items-center gap-2 text-sm text-gray-700 dark:text-gray-300"
			>
				<input
					type="checkbox"
					bind:checked={autoRefresh}
					class="h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
				/>
				Auto-refresh
			</label>
		</div>

		{#if loading}
			<p class="text-sm text-gray-500 dark:text-gray-400" aria-live="polite">Updating alerts…</p>
		{/if}

		{#if feedResponse.windows.length === 0}
			<div
				class="dark:border-dark-border dark:bg-dark-surface rounded-lg border bg-white p-4 text-sm text-gray-500 shadow-sm dark:text-gray-400"
			>
				No windows have been processed yet.
			</div>
		{:else}
			<div class="flex flex-col gap-3">
				{#each feedResponse.windows as window (window.windowStart)}
					<section
						class="dark:border-dark-border dark:bg-dark-surface overflow-hidden rounded-lg border bg-white shadow-sm"
					>
						<header
							class="dark:border-dark-border flex flex-col gap-1 border-b px-4 py-3 sm:flex-row sm:items-center sm:justify-between"
						>
							<div>
								<h2 class="font-medium text-gray-900 dark:text-gray-100">
									{formatTime(window.windowStart)}–{formatTime(window.windowEnd)}
								</h2>
								<p class="text-xs text-gray-500 dark:text-gray-400">
									{formatDate(window.windowStart)}
								</p>
							</div>
							<p class="text-sm text-gray-500 dark:text-gray-400">
								{countFormatter.format(window.addressCount)} addresses · {countFormatter.format(
									window.alertCount
								)} alerts
							</p>
						</header>

						{#if window.alerts.length === 0}
							<div class="px-4 py-2 text-sm text-gray-500 dark:text-gray-400">no alerts</div>
						{:else}
							<div class="dark:divide-dark-border divide-y divide-gray-100">
								{#each window.alerts as alert (`${alert.tail}:${alert.rank}`)}
									<div
										class="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] items-center gap-2 px-4 py-2 text-sm"
									>
										<span class="truncate font-mono text-gray-900 dark:text-gray-100">
											{alert.address}
										</span>
										<span class="text-right text-gray-700 tabular-nums dark:text-gray-300">
											α {alert.alpha.toFixed(3)}
										</span>
										<span
											title={alert.tail === 'high'
												? 'High α — isolated address in a sparse region of address space'
												: 'Low α — address deep inside a dense cluster'}
											class={`justify-self-end rounded-full px-2 py-0.5 text-xs font-medium ${
												alert.tail === 'high'
													? 'bg-red-100 text-red-700 dark:bg-red-950 dark:text-red-300'
													: 'bg-blue-100 text-blue-700 dark:bg-blue-950 dark:text-blue-300'
											}`}
										>
											{alert.tail === 'high' ? 'High' : 'Low'}
										</span>
										<span class="text-right text-gray-500 tabular-nums dark:text-gray-400">
											r² {alert.r2.toFixed(2)}
										</span>
									</div>
								{/each}
							</div>
						{/if}
					</section>
				{/each}
			</div>

			{#if hasOlder && oldestWindow}
				<div class="flex justify-center pt-1">
					<button
						type="button"
						onclick={loadOlderWindows}
						disabled={loadingOlder}
						class="dark:border-dark-border dark:bg-dark-surface rounded-md border border-gray-300 bg-white px-4 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 focus:ring-2 focus:ring-blue-500 focus:outline-none disabled:cursor-not-allowed disabled:opacity-60 dark:text-gray-300 dark:hover:bg-gray-800"
					>
						{loadingOlder ? 'Loading…' : 'Load older windows'}
					</button>
				</div>
			{/if}
		{/if}
	{/if}
</main>
