import type { Chart, ChartArea, Plugin, ChartEvent } from 'chart.js';
import { cancelDrawFrame, requestDrawFrame } from '$lib/utils/animation-frame';

interface CrosshairState {
	mouseX: number | null;
	isMouseOver: boolean;
	showTooltip: boolean;
	tooltipTimeout: NodeJS.Timeout | null;
	hoveredDate: string | null;
	drawFrame: number | null;
}

interface CrosshairOptions {
	enabled: boolean;
	line: {
		color: string;
		width: number;
		dash: number[];
	};
	tooltip: {
		enabled: boolean;
		delay: number;
		backgroundColor: string;
		textColor: string;
		borderColor: string;
		borderWidth: number;
		borderRadius: number;
		padding: number;
		fontSize: number;
		fontFamily: string;
	};
	// Sync options for coordinating crosshairs across multiple charts
	sync?: {
		/** Called when hover state changes (label when hovering, null when leaving) */
		onHover?: (label: string | null) => void;
		/** Returns external hover label if another chart is being hovered */
		getExternalLabel?: () => string | null;
	};
}

// Store mouse state for each chart instance
const chartStates = new WeakMap<Chart, CrosshairState>();
// Helper function to calculate the hovered date from mouse position
function calculateHoveredDate(chart: Chart, mouseX: number): string | null {
	try {
		// Get the x-axis data value from pixel position
		const dataX = chart.scales.x.getValueForPixel(mouseX);
		const labels = chart.data.labels;

		if (typeof dataX === 'number' && labels && labels.length > 0) {
			// Round to nearest label index
			const labelIndex = Math.round(dataX);

			if (labelIndex >= 0 && labelIndex < labels.length) {
				return labels[labelIndex] as string;
			} else {
				// Handle clicks outside data range
				if (labelIndex < 0) {
					return labels[0] as string;
				} else {
					return labels[labels.length - 1] as string;
				}
			}
		}
	} catch (error) {
		console.warn('Error calculating hovered date:', error);
	}

	return null;
}

// Helper function to draw the tooltip
function drawTooltip(
	ctx: CanvasRenderingContext2D,
	mouseX: number,
	date: string,
	chartArea: ChartArea,
	options: CrosshairOptions['tooltip']
) {
	// Set font for text measurement
	ctx.font = `${options.fontSize}px ${options.fontFamily}`;

	// Measure text dimensions
	const textMetrics = ctx.measureText(date);
	const textWidth = textMetrics.width;
	const textHeight = options.fontSize;

	// Calculate tooltip dimensions
	const tooltipWidth = textWidth + options.padding * 2;
	const tooltipHeight = textHeight + options.padding * 2;

	// Calculate tooltip position (center horizontally on the crosshair)
	let tooltipX = mouseX - tooltipWidth / 2;
	let tooltipY = chartArea.top - tooltipHeight - 10; // 10px above the chart

	// Adjust position if tooltip would go outside chart bounds
	if (tooltipX < chartArea.left) {
		tooltipX = chartArea.left + 5;
	} else if (tooltipX + tooltipWidth > chartArea.right) {
		tooltipX = chartArea.right - tooltipWidth - 5;
	}

	// If tooltip would go above chart area, show below crosshair instead
	if (tooltipY < 0) {
		tooltipY = chartArea.top + 10;
	}

	// Save canvas state
	ctx.save();

	// Draw tooltip background
	ctx.fillStyle = options.backgroundColor;
	ctx.strokeStyle = options.borderColor;
	ctx.lineWidth = options.borderWidth;

	// Draw rounded rectangle
	ctx.beginPath();
	ctx.roundRect(tooltipX, tooltipY, tooltipWidth, tooltipHeight, options.borderRadius);
	ctx.fill();
	if (options.borderWidth > 0) {
		ctx.stroke();
	}

	// Draw text
	ctx.fillStyle = options.textColor;
	ctx.textAlign = 'left';
	ctx.textBaseline = 'top';
	ctx.fillText(date, tooltipX + options.padding, tooltipY + options.padding);

	// Restore canvas state
	ctx.restore();
}

function snapCrosshairX(x: number): number {
	return Math.round(x) + 0.5;
}

function scheduleChartDraw(chart: Chart, state: CrosshairState): void {
	if (state.drawFrame !== null) {
		return;
	}

	state.drawFrame = requestDrawFrame(() => {
		state.drawFrame = null;
		chart.draw();
	});
}

export const verticalCrosshairPlugin: Plugin<'line' | 'bar' | 'scatter'> = {
	id: 'verticalCrosshair',

	defaults: {
		enabled: true,
		line: {
			color: 'rgba(128, 128, 128, 0.8)',
			width: 1,
			dash: [5, 5]
		},
		tooltip: {
			enabled: true,
			delay: 500,
			backgroundColor: 'rgba(0, 0, 0, 0.8)',
			textColor: 'white',
			borderColor: 'rgba(100, 100, 100, 0.8)',
			borderWidth: 1,
			borderRadius: 4,
			padding: 8,
			fontSize: 12,
			fontFamily: 'system-ui, sans-serif'
		}
	} as CrosshairOptions,

	afterInit(chart: Chart) {
		// Initialize state for this chart instance
		chartStates.set(chart, {
			mouseX: null,
			isMouseOver: false,
			showTooltip: false,
			tooltipTimeout: null,
			hoveredDate: null,
			drawFrame: null
		});
	},

	afterEvent(chart: Chart, args: { event: ChartEvent }, options: CrosshairOptions) {
		const state = chartStates.get(chart);
		if (!state || !options.enabled) return;

		const event = args.event;

		if (
			event.type === 'mousemove' &&
			event.x !== null &&
			event.x !== undefined &&
			event.y !== null &&
			event.y !== undefined
		) {
			// Check if mouse is within chart area
			const chartArea = chart.chartArea;
			const isInChartArea =
				event.x >= chartArea.left &&
				event.x <= chartArea.right &&
				event.y >= chartArea.top &&
				event.y <= chartArea.bottom;

			if (isInChartArea) {
				state.mouseX = event.x;
				state.isMouseOver = true;

				// Clear existing tooltip timeout
				if (state.tooltipTimeout !== null) {
					clearTimeout(state.tooltipTimeout);
				}

				// Hide tooltip while moving
				if (state.showTooltip) {
					state.showTooltip = false;
					scheduleChartDraw(chart, state);
				}

				// Calculate hovered date
				state.hoveredDate = calculateHoveredDate(chart, event.x);

				// Notify sync callback of hover state change
				options.sync?.onHover?.(state.hoveredDate);

				// Set new timeout to show tooltip if tooltip is enabled
				if (options.tooltip.enabled && state.hoveredDate) {
					state.tooltipTimeout = setTimeout(() => {
						state.showTooltip = true;
						state.tooltipTimeout = null;
						scheduleChartDraw(chart, state);
					}, options.tooltip.delay);
				}

				scheduleChartDraw(chart, state);
			} else if (state.isMouseOver) {
				// Mouse left chart area
				if (state.tooltipTimeout !== null) {
					clearTimeout(state.tooltipTimeout);
					state.tooltipTimeout = null;
				}
				state.isMouseOver = false;
				state.mouseX = null;
				state.showTooltip = false;
				state.hoveredDate = null;

				// Notify sync callback that hover ended
				options.sync?.onHover?.(null);

				scheduleChartDraw(chart, state);
			}
		} else if (event.type === 'mouseout') {
			// Mouse completely left the canvas
			if (state.tooltipTimeout !== null) {
				clearTimeout(state.tooltipTimeout);
				state.tooltipTimeout = null;
			}
			state.isMouseOver = false;
			state.mouseX = null;
			state.showTooltip = false;
			state.hoveredDate = null;

			// Notify sync callback that hover ended
			options.sync?.onHover?.(null);

			scheduleChartDraw(chart, state);
		}
	},

	afterDraw(chart: Chart, args: unknown, options: CrosshairOptions) {
		const state = chartStates.get(chart);
		if (!state || !options.enabled) {
			return;
		}

		const ctx = chart.ctx;
		const chartArea = chart.chartArea;

		// Determine crosshair position: local hover takes precedence over external
		let crosshairX: number | null = null;
		let crosshairLabel: string | null = null;
		let isLocalHover = false;

		if (state.isMouseOver && state.mouseX !== null) {
			// Local hover - use mouse position directly
			crosshairX = state.mouseX;
			crosshairLabel = state.hoveredDate;
			isLocalHover = true;
		} else {
			// Check for external hover from another chart
			const externalLabel = options.sync?.getExternalLabel?.();
			if (externalLabel) {
				// Find the label index and convert to pixel position
				const labels = chart.data.labels;
				if (labels && labels.length > 0) {
					const labelIndex = labels.indexOf(externalLabel);
					if (labelIndex >= 0) {
						crosshairX = chart.scales.x.getPixelForValue(labelIndex);
						crosshairLabel = externalLabel;
					}
				}
			}
		}

		// Don't draw if no crosshair position
		if (crosshairX === null) {
			return;
		}

		const snappedCrosshairX = snapCrosshairX(crosshairX);

		// Save the current canvas state
		ctx.save();

		// Set line style
		ctx.strokeStyle = options.line.color;
		ctx.lineWidth = options.line.width;

		// Set line dash pattern if specified
		if (options.line.dash && options.line.dash.length > 0) {
			ctx.setLineDash(options.line.dash);
		} else {
			ctx.setLineDash([]);
		}

		// Draw vertical line
		ctx.beginPath();
		ctx.moveTo(snappedCrosshairX, chartArea.top);
		ctx.lineTo(snappedCrosshairX, chartArea.bottom);
		ctx.stroke();

		// Restore the canvas state
		ctx.restore();

		// Draw tooltip only for local hover (not external)
		if (isLocalHover && options.tooltip.enabled && state.showTooltip && crosshairLabel) {
			drawTooltip(ctx, snappedCrosshairX, crosshairLabel, chartArea, options.tooltip);
		}
	},

	beforeDestroy(chart: Chart) {
		// Clean up state when chart is destroyed
		const state = chartStates.get(chart);
		if (state && state.tooltipTimeout !== null) {
			clearTimeout(state.tooltipTimeout);
		}
		if (state && state.drawFrame !== null) {
			cancelDrawFrame(state.drawFrame);
		}
		chartStates.delete(chart);
	}
};
