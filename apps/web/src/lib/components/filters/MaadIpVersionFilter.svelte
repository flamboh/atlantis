<script lang="ts">
	import SegmentedControl, {
		type SegmentedControlOption
	} from '#lib/components/common/SegmentedControl.svelte';
	import { MAAD_IP_VERSION_OPTIONS, type MaadIpVersion } from '#lib/types/types.ts';

	const props = $props<{
		ipVersion: MaadIpVersion;
		onIpVersionChange?: (payload: { ipVersion: MaadIpVersion }) => void;
		buttonClass?: string;
	}>();

	const options: SegmentedControlOption<string>[] = MAAD_IP_VERSION_OPTIONS.map((option) => ({
		value: String(option.value),
		label: option.label
	}));

	function handleValueChange(value: string) {
		props.onIpVersionChange?.({ ipVersion: Number(value) as MaadIpVersion });
	}
</script>

<SegmentedControl
	{options}
	value={String(props.ipVersion)}
	onValueChange={handleValueChange}
	ariaLabel="MAAD address family"
	class="grid-cols-2"
	buttonClass={props.buttonClass}
/>
