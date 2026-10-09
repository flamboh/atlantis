<script lang="ts">
	import SegmentedToggle from '#lib/components/common/SegmentedToggle.svelte';
	import ToolbarPopover from '#lib/components/common/ToolbarPopover.svelte';
	import {
		MAAD_IP_VERSION_OPTIONS,
		MAAD_MEASURE_OPTIONS,
		type MaadIpVersion,
		type MaadMeasure
	} from '#lib/types/types.ts';

	let {
		measure,
		ipVersion,
		onMeasureChange,
		onIpVersionChange
	}: {
		measure: MaadMeasure;
		ipVersion: MaadIpVersion;
		onMeasureChange: (measure: MaadMeasure) => void;
		onIpVersionChange: (ipVersion: MaadIpVersion) => void;
	} = $props();

	const measureOptions = MAAD_MEASURE_OPTIONS.map((option) => ({
		...option,
		title:
			option.value === 'addresses'
				? 'Each distinct address counts once'
				: `Weight each address by its ${option.value}; no spectrum`
	}));
	const familyOptions = MAAD_IP_VERSION_OPTIONS.map((option) => ({
		value: String(option.value),
		label: option.label
	}));
	const measureLabel = $derived(
		MAAD_MEASURE_OPTIONS.find((option) => option.value === measure)?.label ?? measure
	);
</script>

<ToolbarPopover
	label="MAAD"
	value={`IPv${ipVersion} · ${measureLabel}`}
	ariaLabel={`MAAD: IPv${ipVersion}, ${measureLabel}`}
	dialogLabel="MAAD options"
	contentClass="w-80"
>
	<div class="grid gap-4 p-3">
		<div class="grid gap-2">
			<p class="text-xs font-medium">Measure</p>
			<SegmentedToggle
				options={measureOptions}
				value={measure}
				onValueChange={onMeasureChange}
				ariaLabel="MAAD measure"
				class="w-full"
				itemClass="flex-1"
			/>
			<p class="text-muted-foreground text-xs leading-4">
				Addresses counts each address once and includes the spectrum. Packets and Bytes weight each
				address by its traffic.
			</p>
		</div>
		<div class="grid gap-2">
			<p class="text-xs font-medium">Address family</p>
			<SegmentedToggle
				options={familyOptions}
				value={String(ipVersion)}
				onValueChange={(value) => onIpVersionChange(Number(value) as MaadIpVersion)}
				ariaLabel="MAAD address family"
				class="w-full"
				itemClass="flex-1"
			/>
		</div>
	</div>
</ToolbarPopover>
