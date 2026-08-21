export interface RangeSelectionState {
	sourceChartId: string;
	startLabel: string;
	endLabel: string;
}

class RangeSelection {
	selection = $state<RangeSelectionState | null>(null);

	set(selection: RangeSelectionState) {
		this.selection = selection;
	}

	clear() {
		this.selection = null;
	}
}

export const rangeSelection = new RangeSelection();
