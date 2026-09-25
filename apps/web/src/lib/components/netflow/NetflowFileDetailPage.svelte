<script lang="ts">
	import { afterNavigate } from '$app/navigation';
	import { getNetflowFileDetailLoader } from '$lib/components/netflow/file-detail-loader.svelte';
	import NetflowFileHeader from '$lib/components/netflow/NetflowFileHeader.svelte';
	import NetflowFileLoadingSkeleton from '$lib/components/netflow/NetflowFileLoadingSkeleton.svelte';
	import NetflowFileMessageCard from '$lib/components/netflow/NetflowFileMessageCard.svelte';
	import NetflowFileRouterCard from '$lib/components/netflow/NetflowFileRouterCard.svelte';
	import SegmentedControl from '$lib/components/common/SegmentedControl.svelte';
	import MaadMeasureFilter from '$lib/components/filters/MaadMeasureFilter.svelte';
	import {
		DEFAULT_MAAD_IP_VERSION,
		DEFAULT_MAAD_MEASURE,
		MAAD_IP_VERSION_OPTIONS,
		maadMeasureHasSpectrum,
		type FlowDirection,
		type MaadIpVersion,
		type MaadMeasure
	} from '$lib/types/types';
	import {
		createDateFromPSTComponents,
		epochToPSTComponents,
		formatTimestampAsPST
	} from '$lib/utils/timezone';
	import { onMount } from 'svelte';

	type NetflowFileDetailData = {
		dataset: string;
		slug: string;
		direction: FlowDirection;
		fileInfo: {
			year: string;
			month: string;
			day: string;
			hour: string;
			minute: string;
			filename: string;
		};
	};

	let { data }: { data: NetflowFileDetailData } = $props();
	let loader = $state.raw<ReturnType<typeof getNetflowFileDetailLoader> | null>(null);
	let maadIpVersion = $state<MaadIpVersion>(DEFAULT_MAAD_IP_VERSION);
	let maadMeasure = $state<MaadMeasure>(DEFAULT_MAAD_MEASURE);
	const showSpectrum = $derived(maadMeasureHasSpectrum(maadMeasure));

	const formatCount = (value: number | null | undefined) =>
		typeof value === 'number' && Number.isFinite(value) ? value.toLocaleString() : 'N/A';

	function getNextSlug(slug: string) {
		if (!slug || slug.length !== 12 || !/^\d{12}$/.test(slug)) {
			return slug;
		}
		const year = parseInt(slug.slice(0, 4), 10);
		const month = parseInt(slug.slice(4, 6), 10);
		const day = parseInt(slug.slice(6, 8), 10);
		const hour = parseInt(slug.slice(8, 10), 10);
		const minute = parseInt(slug.slice(10, 12), 10);
		const currentDate = createDateFromPSTComponents(year, month, day, hour, minute);
		const nextDate = new Date(currentDate.getTime() + 5 * 60 * 1000);
		const nextPST = epochToPSTComponents(Math.floor(nextDate.getTime() / 1000));

		return `${nextPST.year}${String(nextPST.month).padStart(2, '0')}${String(nextPST.day).padStart(2, '0')}${String(nextPST.hours).padStart(2, '0')}${String(nextPST.minutes).padStart(2, '0')}`;
	}

	const nextSlug = $derived(getNextSlug(data.slug));

	function syncLoader() {
		loader = getNetflowFileDetailLoader(
			data.dataset,
			data.slug,
			data.direction,
			maadIpVersion,
			maadMeasure
		);
		loader.refresh();
	}

	function refreshLoader() {
		loader?.refresh();
	}

	function handleMaadIpVersionChange(nextIpVersion: MaadIpVersion) {
		if (nextIpVersion === maadIpVersion) {
			return;
		}
		maadIpVersion = nextIpVersion;
		syncLoader();
	}

	function handleMaadMeasureChange({ measure }: { measure: MaadMeasure }) {
		if (measure === maadMeasure) {
			return;
		}
		maadMeasure = measure;
		syncLoader();
	}

	onMount(() => {
		syncLoader();
	});

	afterNavigate(() => {
		syncLoader();
	});
</script>

<div class="mx-auto max-w-[95vw] px-2 py-2 sm:px-2 lg:px-4">
	<NetflowFileHeader
		dataset={data.dataset}
		{nextSlug}
		direction={data.direction}
		filename={data.fileInfo.filename}
		year={data.fileInfo.year}
		month={data.fileInfo.month}
		day={data.fileInfo.day}
		hour={data.fileInfo.hour}
		minute={data.fileInfo.minute}
		processedAt={loader?.processedAt
			? formatTimestampAsPST(Date.parse(loader.processedAt))
			: loader?.loading
				? 'Loading...'
				: 'N/A'}
	/>

	<div class="mb-2 flex flex-wrap items-center gap-x-4 gap-y-2">
		<div class="flex items-center gap-2">
			<span class="text-foreground text-sm font-medium">MAAD address family:</span>
			<SegmentedControl
				options={MAAD_IP_VERSION_OPTIONS.map((option) => ({
					value: String(option.value),
					label: option.label
				}))}
				value={String(maadIpVersion)}
				onValueChange={(value) => handleMaadIpVersionChange(Number(value) as MaadIpVersion)}
				ariaLabel="Select MAAD IP address family"
				buttonClass="sm:min-w-32"
			/>
		</div>
		<div class="flex items-center gap-2">
			<span class="text-foreground text-sm font-medium">MAAD measure:</span>
			<MaadMeasureFilter measure={maadMeasure} onMeasureChange={handleMaadMeasureChange} />
		</div>
	</div>

	{#if loader?.error && !loader.hasRows}
		<NetflowFileMessageCard
			tone="danger"
			message={`Failed to load file summary: ${loader.error}`}
			actionLabel="Retry Summary"
			action={refreshLoader}
		/>
	{:else if !loader || !loader.hasLoadedOnce || (!loader.hasRows && loader.loading && loader.skeletonVisible)}
		<NetflowFileLoadingSkeleton count={loader?.skeletonCount ?? 2} />
	{:else if !loader.hasRows && loader.loading}
		<NetflowFileMessageCard message="Loading file summary..." />
	{:else if !loader.hasRows}
		<NetflowFileMessageCard message="No database summary is available for this file." />
	{:else}
		<div class="space-y-3">
			{#if loader.error}
				<NetflowFileMessageCard
					tone="danger"
					message={`Failed to refresh file summary: ${loader.error}`}
					actionLabel="Retry Summary"
					action={refreshLoader}
				/>
			{/if}
			{#each loader.rows as row (row.key)}
				<NetflowFileRouterCard {row} {showSpectrum} {formatCount} {formatTimestampAsPST} />
			{/each}
		</div>
	{/if}
</div>
