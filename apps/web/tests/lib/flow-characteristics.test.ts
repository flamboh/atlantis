import { describe, expect, it } from 'vitest';
import {
	createRequestGate,
	getSourceLineDash
} from '../../src/lib/components/charts/flow-characteristics';

describe('flow characteristics request coordination', () => {
	it('rejects an in-flight response after all sources are deselected', () => {
		const gate = createRequestGate();
		const inFlightRequest = gate.begin();

		gate.begin(); // loadData's empty-source early return

		expect(gate.isCurrent(inFlightRequest)).toBe(false);
	});

	it('rejects an in-flight response while a new dataset has not loaded its sources', () => {
		const gate = createRequestGate();
		const previousDatasetRequest = gate.begin();

		gate.begin(); // loadData's routersLoaded=false early return

		expect(gate.isCurrent(previousDatasetRequest)).toBe(false);
	});

	it('uses a distinct line pattern for each fallback source', () => {
		expect(getSourceLineDash(0, true)).not.toEqual(getSourceLineDash(1, true));
		expect(getSourceLineDash(0, false)).toEqual([]);
	});
});
