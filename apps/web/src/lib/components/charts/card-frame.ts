import { createContext } from 'svelte';

export type CardFrame = {
	readonly id: string;
	readonly title: string;
	readonly first: boolean;
	readonly last: boolean;
	readonly resizable: boolean;
	move(offset: number): void;
	resize(offset: number): void;
};

export const [getCardFrame, setCardFrame, hasCardFrame] = createContext<CardFrame>();

export function optionalCardFrame(): CardFrame | null {
	return hasCardFrame() ? getCardFrame() : null;
}
