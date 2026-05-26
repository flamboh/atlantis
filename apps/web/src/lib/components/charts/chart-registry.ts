import {
	CategoryScale,
	Chart as ChartJS,
	Filler,
	Legend,
	LineController,
	LineElement,
	LinearScale,
	PointElement,
	ScatterController,
	Tooltip
} from 'chart.js';
import { verticalCrosshairPlugin } from './crosshair-plugin';

let registered = false;

export function ensureChartRegistry(): void {
	if (registered) {
		return;
	}

	ChartJS.register(
		CategoryScale,
		Filler,
		Legend,
		LineController,
		LineElement,
		LinearScale,
		PointElement,
		ScatterController,
		Tooltip,
		verticalCrosshairPlugin
	);
	registered = true;
}

ensureChartRegistry();

export { ChartJS as Chart };
