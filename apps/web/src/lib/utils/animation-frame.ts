export function requestDrawFrame(callback: FrameRequestCallback): number {
	if (typeof requestAnimationFrame === 'function') {
		return requestAnimationFrame(callback);
	}

	return setTimeout(() => callback(Date.now()), 16) as unknown as number;
}

export function cancelDrawFrame(frame: number): void {
	if (typeof cancelAnimationFrame === 'function') {
		cancelAnimationFrame(frame);
		return;
	}

	clearTimeout(frame);
}
