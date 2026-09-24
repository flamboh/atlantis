<script lang="ts">
	import SegmentedControl, {
		type SegmentedControlOption
	} from '$lib/components/common/SegmentedControl.svelte';
	import { MAAD_MEASURE_OPTIONS, type MaadMeasure } from '$lib/types/types';

	const props = $props<{
		measure: MaadMeasure;
		onMeasureChange?: (payload: { measure: MaadMeasure }) => void;
	}>();

	const options: SegmentedControlOption<MaadMeasure>[] = MAAD_MEASURE_OPTIONS.map((option) => ({
		value: option.value,
		label: option.label,
		title:
			option.value === 'addresses'
				? 'Each distinct address counts once'
				: `Weight each address by its ${option.value}; no spectrum`
	}));

	function handleValueChange(value: MaadMeasure) {
		props.onMeasureChange?.({ measure: value });
	}
</script>

<SegmentedControl
	{options}
	value={props.measure}
	onValueChange={handleValueChange}
	ariaLabel="MAAD measure"
/>
