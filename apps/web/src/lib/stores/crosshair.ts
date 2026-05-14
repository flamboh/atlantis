import type { Chart } from 'chart.js';
import { cancelDrawFrame, requestDrawFrame } from '$lib/utils/animation-frame';

export interface CrosshairSnapshot {
	label: string | null;
	sourceChartId: string | null;
}

/**
 * Centralized crosshair store that manages synchronized crosshairs across multiple charts.
 * When one chart is hovered, this store triggers redraws on all other registered charts
 * so they can display crosshairs at the same x-axis position.
 */
class CrosshairStore {
	private charts = new Map<string, Chart>();
	private drawFrames = new Map<Chart, number>();
	private _hoveredLabel: string | null = null;
	private _sourceChartId: string | null = null;
	private listeners = new Set<(snapshot: CrosshairSnapshot) => void>();

	private notify(): void {
		const snapshot: CrosshairSnapshot = {
			label: this._hoveredLabel,
			sourceChartId: this._sourceChartId
		};
		this.listeners.forEach((listener) => {
			listener(snapshot);
		});
	}

	/**
	 * Register a chart instance with the store.
	 * Call this when the chart is created/mounted.
	 */
	register(id: string, chart: Chart): void {
		this.charts.set(id, chart);
	}

	/**
	 * Unregister a chart instance from the store.
	 * Call this when the chart is destroyed/unmounted.
	 */
	unregister(id: string): void {
		const chart = this.charts.get(id);
		if (chart) {
			this.cancelScheduledDraw(chart);
		}
		this.charts.delete(id);
	}

	private cancelScheduledDraw(chart: Chart): void {
		const frame = this.drawFrames.get(chart);
		if (frame !== undefined) {
			cancelDrawFrame(frame);
			this.drawFrames.delete(chart);
		}
	}

	private scheduleDraw(chart: Chart): void {
		if (this.drawFrames.has(chart)) {
			return;
		}

		const frame = requestDrawFrame(() => {
			this.drawFrames.delete(chart);
			chart.draw();
		});
		this.drawFrames.set(chart, frame);
	}

	subscribe(listener: (snapshot: CrosshairSnapshot) => void): () => void {
		this.listeners.add(listener);
		listener({
			label: this._hoveredLabel,
			sourceChartId: this._sourceChartId
		});
		return () => {
			this.listeners.delete(listener);
		};
	}

	/**
	 * Update the hover state and trigger redraws on all other charts.
	 * Called by the crosshair plugin when mouse moves over a chart.
	 */
	setHover(label: string | null, sourceId: string): void {
		if (this._hoveredLabel === label && this._sourceChartId === sourceId) {
			return;
		}
		this._hoveredLabel = label;
		this._sourceChartId = sourceId;
		this.notify();

		// Directly redraw all OTHER charts - no reactive overhead
		this.charts.forEach((chart, id) => {
			if (id !== sourceId) {
				this.scheduleDraw(chart);
			}
		});
	}

	/**
	 * Clear the hover state and redraw all charts.
	 * Called when mouse leaves a chart entirely.
	 */
	clearHover(): void {
		if (this._hoveredLabel === null && this._sourceChartId === null) {
			return;
		}
		this._hoveredLabel = null;
		this._sourceChartId = null;
		this.notify();

		// Redraw all charts to clear crosshairs
		this.charts.forEach((chart) => {
			this.scheduleDraw(chart);
		});
	}

	/**
	 * Get the external hover label for a specific chart.
	 * Returns the hovered label if this chart is NOT the source of the hover,
	 * otherwise returns null (so the chart uses its own local crosshair).
	 */
	getExternalLabel(chartId: string): string | null {
		if (this._sourceChartId !== chartId) {
			return this._hoveredLabel;
		}
		return null;
	}

	/**
	 * Get the current hovered label (for debugging/inspection).
	 */
	get hoveredLabel(): string | null {
		return this._hoveredLabel;
	}

	/**
	 * Get the current source chart ID (for debugging/inspection).
	 */
	get sourceChartId(): string | null {
		return this._sourceChartId;
	}
}

// Export singleton instance
export const crosshairStore = new CrosshairStore();
