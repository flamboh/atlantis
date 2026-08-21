<script lang="ts">
	import { afterNavigate } from '$app/navigation';
	import { untrack } from 'svelte';
	import type { PageProps } from './$types';
	import SegmentedControl from '$lib/components/common/SegmentedControl.svelte';
	import DatasetTabs from '$lib/components/datasets/DatasetTabs.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Card, CardContent, CardHeader, CardTitle } from '$lib/components/ui/card';
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow
	} from '$lib/components/ui/table';
	import type { AlertHorizon, AlertsFeedResponse, AlertSort, AlertTail } from '$lib/types/types';

	type TailSelection = AlertTail;
	type ErrorResponse = { data: null; error: string };

	const PAGE_SIZE = 100;
	const MAX_LIMIT = 500;
	const REFRESH_INTERVAL_MS = 30_000;
	const LIVE_WINDOW_AGE_MS = 15 * 60_000;
	const NEW_ADDRESS_AGE_SECONDS = 15 * 60;
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
		<h1 class="text-foreground text-2xl font-semibold">Singularity alerts</h1>
		<p class="text-foreground mt-1 text-sm">{selectedDatasetLabel}</p>
		<p class="text-muted-foreground mt-2 text-sm" aria-live="polite">
			{statusText}
		</p>
	</header>
	<DatasetTabs datasetId={data.selectedDataset} active="alerts" />

	{#if fetchError}
		<Card
			class="border-destructive bg-destructive/10 text-destructive gap-0 py-4 ring-0"
			role="alert"
		>
			<CardContent>{fetchError}</CardContent>
		</Card>
	{/if}

	{#if !feedResponse.feed.present}
		<Card class="text-muted-foreground gap-3 rounded-lg border py-4 shadow-sm ring-0">
			<CardHeader class="px-4">
				<CardTitle class="text-foreground font-medium">The alert feed is not running</CardTitle>
				<p class="text-sm">Start it for this dataset with:</p>
			</CardHeader>
			<CardContent class="px-4">
				<div
					class="bg-muted flex flex-col gap-2 rounded-md p-3 sm:flex-row sm:items-center sm:justify-between"
				>
					<code class="text-foreground overflow-x-auto font-mono text-sm">{feedCommand}</code>
					<Button onclick={copyCommand} size="sm" class="h-7 shrink-0 px-3">
						{copied ? 'Copied' : 'Copy'}
					</Button>
				</div>
				{#if copyError}
					<p class="text-destructive mt-2 text-sm" role="alert">{copyError}</p>
				{/if}
			</CardContent>
		</Card>
	{:else}
		<div class="flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
			<div class="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-end">
				<div class="flex flex-col gap-1">
					<span class="text-muted-foreground text-xs font-medium">Alpha</span>
					<SegmentedControl
						options={TAIL_OPTIONS}
						value={selectedTail}
						onValueChange={selectTail}
						class="grid-cols-2"
						ariaLabel="Filter alerts by alpha tail"
					/>
				</div>

				<div class="flex flex-col gap-1">
					<span class="text-muted-foreground text-xs font-medium">Horizon</span>
					<SegmentedControl
						options={HORIZON_OPTIONS}
						value={selectedHorizon}
						onValueChange={selectHorizon}
						class="grid-cols-4"
						ariaLabel="Select alert horizon"
					/>
				</div>

				<div class="flex flex-col gap-1">
					<span class="text-muted-foreground text-xs font-medium">Sort</span>
					<SegmentedControl
						options={SORT_OPTIONS}
						value={selectedSort}
						onValueChange={selectSort}
						class="grid-cols-2"
						ariaLabel="Sort alert addresses"
					/>
				</div>

				{#if feedResponse.feed.present && feedResponse.feed.thresholds}
					<div class="flex flex-col gap-1">
						<span class="text-muted-foreground text-xs font-medium">Thresholds</span>
						<p
							class="text-muted-foreground flex min-h-7 items-center pb-1 text-sm tabular-nums"
							title="The feed records an address when its alpha crosses either calibrated bound"
						>
							α ≥ {feedResponse.feed.thresholds.high} · α ≤ {feedResponse.feed.thresholds.low}
						</p>
					</div>
				{/if}
			</div>
		</div>

		{#if feedResponse.addresses.length === 0}
			<Card class="text-muted-foreground gap-0 rounded-lg border py-3 text-sm shadow-sm ring-0">
				<CardContent class="px-4">
					No anomalous addresses in the last {selectedHorizon}.
				</CardContent>
			</Card>
		{:else}
			<section
				class={`border-border bg-card text-card-foreground overflow-x-auto rounded-lg border shadow-sm transition-opacity ${loading ? 'opacity-60' : ''}`}
				aria-label="Anomalous addresses"
				aria-busy={loading}
			>
				<Table class="w-full text-sm">
					<TableHeader>
						<TableRow class="text-muted-foreground text-xs font-medium hover:bg-transparent">
							<TableHead class="h-auto w-full px-4 py-2 text-left font-medium">Address</TableHead>
							<TableHead
								class={`px-4 py-2 text-right font-medium whitespace-nowrap ${
									selectedSort === 'extreme' ? 'text-foreground' : ''
								}`}
							>
								Peak α
							</TableHead>
							<TableHead
								class={`px-4 py-2 text-right font-medium whitespace-nowrap ${
									selectedSort === 'recent' ? 'text-foreground' : ''
								}`}
							>
								Latest α
							</TableHead>
							<TableHead class="h-auto px-4 py-2 text-right font-medium">First seen</TableHead>
							<TableHead class="h-auto px-4 py-2 text-right font-medium">Last seen</TableHead>
							<TableHead
								class="h-auto px-4 py-2 text-right font-medium"
								title="Windows flagged in this horizon"
							>
								Flagged
							</TableHead>
							<TableHead class="h-auto px-4 py-2 text-right font-medium">r²</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						{#each feedResponse.addresses as alert (alert.address)}
							<TableRow class="hover:bg-transparent">
								<TableCell class="px-4 py-3">
									<div class="flex min-w-0 items-center gap-2">
										<span class="text-foreground truncate font-mono">
											{alert.address}
										</span>
										{#if isNewAddress(alert.firstSeen)}
											<span
												title={`First flagged ${formatRelativeTime(alert.firstSeen)} — never seen in the retained history before that`}
												class="bg-accent text-accent-foreground shrink-0 rounded-full px-2 py-0.5 text-xs font-medium"
											>
												new
											</span>
										{/if}
									</div>
								</TableCell>
								<TableCell
									class={`px-4 py-3 text-right tabular-nums ${
										selectedSort === 'extreme'
											? 'text-foreground font-medium'
											: 'text-muted-foreground/70'
									}`}
								>
									{alert.peakAlpha.toFixed(3)}
								</TableCell>
								<TableCell
									class={`px-4 py-3 text-right tabular-nums ${
										selectedSort === 'recent'
											? 'text-foreground font-medium'
											: 'text-muted-foreground/70'
									}`}
								>
									{alert.latestAlpha.toFixed(3)}
								</TableCell>
								<TableCell
									title={dateTimeFormatter.format(new Date(alert.firstSeen * 1000))}
									class="text-muted-foreground px-4 py-3 text-right whitespace-nowrap tabular-nums"
								>
									{formatRelativeTime(alert.firstSeen)}
								</TableCell>
								<TableCell
									title={dateTimeFormatter.format(new Date(alert.lastSeen * 1000))}
									class="text-muted-foreground px-4 py-3 text-right whitespace-nowrap tabular-nums"
								>
									{formatRelativeTime(alert.lastSeen)}
								</TableCell>
								<TableCell
									title={`flagged in ${countFormatter.format(alert.timesFlagged)} windows in this horizon`}
									class="text-foreground px-4 py-3 text-right tabular-nums"
								>
									{countFormatter.format(alert.timesFlagged)}×
								</TableCell>
								<TableCell class="text-muted-foreground px-4 py-3 text-right tabular-nums">
									{alert.peakR2.toFixed(2)}
								</TableCell>
							</TableRow>
						{/each}
					</TableBody>
				</Table>
			</section>
		{/if}

		<div class="flex flex-col items-center gap-2 pt-1">
			{#if canShowMore}
				<Button
					variant="outline"
					onclick={showMore}
					disabled={loadingMore}
					class="px-4 disabled:cursor-not-allowed disabled:opacity-60"
				>
					{loadingMore ? 'Loading…' : 'Show more'}
				</Button>
			{/if}
			<p class="text-muted-foreground text-xs">
				showing {countFormatter.format(feedResponse.addresses.length)} of {countFormatter.format(
					feedResponse.totalAddresses
				)}
			</p>
		</div>
	{/if}
</main>
