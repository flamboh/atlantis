<script lang="ts">
	import SegmentedControl, {
		type SegmentedControlOption
	} from '#lib/components/common/SegmentedControl.svelte';
	import { FLOW_DIRECTION_OPTIONS, type FlowDirection } from '#lib/types/types.ts';

	const props = $props<{
		direction: FlowDirection;
		onDirectionChange?: (payload: { direction: FlowDirection }) => void;
		buttonClass?: string;
	}>();

	const options: SegmentedControlOption<FlowDirection>[] = FLOW_DIRECTION_OPTIONS.map((option) => ({
		value: option.value,
		label: option.label,
		title: option.description
	}));

	function handleValueChange(value: FlowDirection) {
		props.onDirectionChange?.({ direction: value });
	}
</script>

<SegmentedControl
	{options}
	value={props.direction}
	onValueChange={handleValueChange}
	ariaLabel="Traffic direction"
	class="grid-cols-5"
	buttonClass={props.buttonClass}
/>
