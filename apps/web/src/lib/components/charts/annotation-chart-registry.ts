import annotationPlugin from 'chartjs-plugin-annotation';
import { Chart } from './chart-registry';

let annotationRegistered = false;

export function ensureAnnotationChartRegistry(): void {
	if (annotationRegistered) {
		return;
	}

	Chart.register(annotationPlugin);
	annotationRegistered = true;
}

ensureAnnotationChartRegistry();

export { Chart };
