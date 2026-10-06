export interface CrosshairSnapshot {
	label: string | null;
	sourceChartId: string | null;
}

class CrosshairStore {
	private snapshot: CrosshairSnapshot = { label: null, sourceChartId: null };
	private listeners = new Set<(snapshot: CrosshairSnapshot) => void>();
	subscribe(listener: (snapshot: CrosshairSnapshot) => void): () => void {
		this.listeners.add(listener);
		listener(this.snapshot);
		return () => {
			this.listeners.delete(listener);
		};
	}
	setHover(label: string | null, sourceId: string): void {
		if (this.snapshot.label === label && this.snapshot.sourceChartId === sourceId) return;
		this.snapshot = { label, sourceChartId: sourceId };
		this.listeners.forEach((listener) => listener(this.snapshot));
	}
	clearHover(): void {
		if (this.snapshot.label === null && this.snapshot.sourceChartId === null) return;
		this.snapshot = { label: null, sourceChartId: null };
		this.listeners.forEach((listener) => listener(this.snapshot));
	}
	getExternalLabel(chartId: string): string | null {
		return this.snapshot.sourceChartId === chartId ? null : this.snapshot.label;
	}
	get hoveredLabel() {
		return this.snapshot.label;
	}
	get sourceChartId() {
		return this.snapshot.sourceChartId;
	}
}

export const crosshairStore = new CrosshairStore();
