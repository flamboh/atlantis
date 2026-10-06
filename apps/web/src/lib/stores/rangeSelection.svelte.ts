export interface RangeSelectionState {
	sourceChartId: string;
	startLabel: string;
	endLabel: string;
}

class RangeSelection {
	selection = $state<RangeSelectionState | null>(null);
	private listeners = new Set<() => void>();

	subscribe(listener: () => void): () => void {
		this.listeners.add(listener);
		listener();
		return () => {
			this.listeners.delete(listener);
		};
	}

	set(selection: RangeSelectionState) {
		this.selection = selection;
		this.listeners.forEach((listener) => listener());
	}

	clear() {
		this.selection = null;
		this.listeners.forEach((listener) => listener());
	}
}

export const rangeSelection = new RangeSelection();
