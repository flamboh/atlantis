export interface RequestGate {
	begin(): number;
	isCurrent(token: number): boolean;
}

export function createRequestGate(): RequestGate {
	let currentToken = 0;
	return {
		begin: () => ++currentToken,
		isCurrent: (token) => token === currentToken
	};
}

const SOURCE_LINE_DASHES: number[][] = [[], [8, 3], [3, 3], [10, 3, 2, 3]];

export function getSourceLineDash(sourceIndex: number, multipleSources: boolean): number[] {
	if (!multipleSources) return [];
	return SOURCE_LINE_DASHES[sourceIndex % SOURCE_LINE_DASHES.length] ?? [];
}
