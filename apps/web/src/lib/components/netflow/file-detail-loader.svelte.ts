import type {
	FileIpCounts,
	FlowScope,
	NetflowFileDetailsResponse,
	NetflowFileSummaryRecord,
	SpectrumData,
	StructureFunctionData
} from '$lib/types/types';
import { SvelteMap } from 'svelte/reactivity';

const SUMMARY_SKELETON_DELAY_MS = 150;
const FILE_DETAIL_CACHE_LIMIT = 8;

type Side = 'source' | 'destination';
type ResourceKind = 'ipCounts' | 'structure' | 'spectrum';

type ResourceValueMap = {
	ipCounts: FileIpCounts;
	structure: StructureFunctionData;
	spectrum: SpectrumData;
};

type ResourceState<T> = {
	data: T | null;
	loading: boolean;
	error: string | null;
	missing: boolean;
	loaded: boolean;
	requestToken: number;
};

type RouterSideState = {
	ipCounts: ResourceState<FileIpCounts>;
	structure: ResourceState<StructureFunctionData>;
	spectrum: ResourceState<SpectrumData>;
};

type RouterState = {
	summary: NetflowFileSummaryRecord;
	source: RouterSideState;
	destination: RouterSideState;
};

export type FileDetailResourceView<T> = {
	data: T | null;
	loading: boolean;
	error: string | null;
	missing: boolean;
	refresh: () => void;
};

export type NetflowFileRouterRow = {
	key: string;
	router: string;
	summary: NetflowFileSummaryRecord;
	refreshSummary: () => void;
	source: {
		ipCounts: FileDetailResourceView<FileIpCounts>;
		structure: FileDetailResourceView<StructureFunctionData>;
		spectrum: FileDetailResourceView<SpectrumData>;
	};
	destination: {
		ipCounts: FileDetailResourceView<FileIpCounts>;
		structure: FileDetailResourceView<StructureFunctionData>;
		spectrum: FileDetailResourceView<SpectrumData>;
	};
};

function buildLoaderKey(dataset: string, slug: string, flowScope: FlowScope): string {
	return `${dataset}:${slug}:${flowScope.srcVisibility}:${flowScope.dstVisibility}`;
}

function buildScopeSearchParams(flowScope: FlowScope): Record<string, string> {
	if (flowScope.srcVisibility === 'all' && flowScope.dstVisibility === 'all') {
		return {};
	}

	return {
		srcVisibility: flowScope.srcVisibility,
		dstVisibility: flowScope.dstVisibility
	};
}

async function readErrorMessage(response: Response, fallback: string): Promise<string> {
	try {
		const payload = (await response.json()) as { error?: string };
		if (payload?.error) {
			return payload.error;
		}
	} catch {
		// Keep fallback.
	}

	return fallback;
}

class NetflowFileDetailLoader {
	readonly dataset: string;
	readonly slug: string;
	readonly flowScope: FlowScope;
	readonly key: string;

	private _loading = $state(false);
	private _error = $state<string | null>(null);
	private _skeletonVisible = $state(false);
	private _skeletonCount = $state(2);
	private _routers = $state<RouterState[]>([]);
	private _hasLoadedOnce = $state(false);
	private _loadToken = 0;
	private _lastAccessedAt = Date.now();
	private summarySkeletonDelayId: ReturnType<typeof setTimeout> | null = null;

	constructor(dataset: string, slug: string, flowScope: FlowScope) {
		this.dataset = dataset;
		this.slug = slug;
		this.flowScope = flowScope;
		this.key = buildLoaderKey(dataset, slug, flowScope);
	}

	get loading() {
		return this._loading;
	}

	get error() {
		return this._error;
	}

	get skeletonVisible() {
		return this._skeletonVisible;
	}

	get skeletonCount() {
		return this._skeletonCount;
	}

	get rows(): NetflowFileRouterRow[] {
		return this._routers.map((routerState) => this.createRow(routerState));
	}

	get hasLoadedOnce() {
		return this._hasLoadedOnce;
	}

	get hasRows() {
		return this._routers.length > 0;
	}

	get processedAt() {
		return this._routers[0]?.summary.processed_at ?? null;
	}

	get lastAccessedAt() {
		return this._lastAccessedAt;
	}

	touch() {
		this._lastAccessedAt = Date.now();
	}

	refresh() {
		void this.loadSummary();
	}

	private createRow(routerState: RouterState): NetflowFileRouterRow {
		const router = routerState.summary.router;

		return {
			key: `${this.key}:${router}`,
			router,
			summary: routerState.summary,
			refreshSummary: () => this.refresh(),
			source: {
				ipCounts: this.createResourceView(router, 'source', 'ipCounts'),
				structure: this.createResourceView(router, 'source', 'structure'),
				spectrum: this.createResourceView(router, 'source', 'spectrum')
			},
			destination: {
				ipCounts: this.createResourceView(router, 'destination', 'ipCounts'),
				structure: this.createResourceView(router, 'destination', 'structure'),
				spectrum: this.createResourceView(router, 'destination', 'spectrum')
			}
		};
	}

	private createResourceView<K extends ResourceKind>(
		router: string,
		side: Side,
		kind: K
	): FileDetailResourceView<ResourceValueMap[K]> {
		const slot = this.getSlot(router, side, kind) as ResourceState<ResourceValueMap[K]>;

		return {
			data: slot.data,
			loading: slot.loading,
			error: slot.error,
			missing: slot.missing,
			refresh: () => {
				void this.refreshResource(router, side, kind);
			}
		};
	}

	private getRouterState(router: string) {
		return this._routers.find((entry) => entry.summary.router === router) ?? null;
	}

	private getSideState(router: string, side: Side) {
		const routerState = this.getRouterState(router);
		if (!routerState) {
			return null;
		}

		return side === 'source' ? routerState.source : routerState.destination;
	}

	private getSlot<K extends ResourceKind>(router: string, side: Side, kind: K): RouterSideState[K] {
		const sideState = this.getSideState(router, side);
		if (!sideState) {
			throw new Error(`Unknown router "${router}" for ${this.key}`);
		}

		return sideState[kind];
	}

	private startSummaryLoading() {
		this.clearSummaryLoadDelay();
		this._loading = true;
		this._skeletonVisible = false;
		this.summarySkeletonDelayId = setTimeout(() => {
			if (this._loading && !this.hasRows) {
				this._skeletonVisible = true;
			}
		}, SUMMARY_SKELETON_DELAY_MS);
	}

	private finishSummaryLoading() {
		this.clearSummaryLoadDelay();
		this._loading = false;
		this._skeletonVisible = false;
	}

	private clearSummaryLoadDelay() {
		if (this.summarySkeletonDelayId !== null) {
			clearTimeout(this.summarySkeletonDelayId);
			this.summarySkeletonDelayId = null;
		}
	}

	private buildRouterState(summary: NetflowFileDetailsResponse['routers'][number]): RouterState {
		return {
			summary: summary.summary,
			source: {
				ipCounts: this.createLoadedResourceState(summary.ipCountsSource),
				structure: this.createLoadedResourceState(summary.structureSource),
				spectrum: this.createLoadedResourceState(summary.spectrumSource)
			},
			destination: {
				ipCounts: this.createLoadedResourceState(summary.ipCountsDestination),
				structure: this.createLoadedResourceState(summary.structureDestination),
				spectrum: this.createLoadedResourceState(summary.spectrumDestination)
			}
		};
	}

	private createLoadedResourceState<T>(data: T | null): ResourceState<T> {
		return {
			data,
			loading: false,
			error: null,
			missing: data === null,
			loaded: true,
			requestToken: 0
		};
	}

	private async loadSummary() {
		const token = ++this._loadToken;

		this._skeletonCount = Math.max(this._routers.length, this._skeletonCount, 2);
		this.startSummaryLoading();
		this._error = null;

		try {
			const response = await fetch(
				`/api/netflow/files/${this.slug}/details?${new URLSearchParams({
					dataset: this.dataset,
					...buildScopeSearchParams(this.flowScope)
				}).toString()}`
			);

			if (!response.ok) {
				throw new Error(await readErrorMessage(response, 'Failed to load file summary'));
			}

			const result = (await response.json()) as NetflowFileDetailsResponse;
			if (token !== this._loadToken) {
				return;
			}

			this._routers = result.routers.map((router) => this.buildRouterState(router));
			this._skeletonCount = Math.max(result.routers.length, 2);
			this._hasLoadedOnce = true;
			this.finishSummaryLoading();
		} catch (error) {
			if (token !== this._loadToken) {
				return;
			}

			this._hasLoadedOnce = true;
			this._error = error instanceof Error ? error.message : 'Unknown error occurred';
			this.finishSummaryLoading();
		}
	}

	private async refreshResource<K extends ResourceKind>(router: string, side: Side, kind: K) {
		const slot = this.getSlot(router, side, kind);
		const token = slot.requestToken + 1;

		slot.requestToken = token;
		slot.loading = true;
		slot.error = null;

		try {
			const response = await fetch(this.buildResourceUrl(kind, router, side));
			if (token !== slot.requestToken) {
				return;
			}

			if (response.status === 404) {
				const message = await readErrorMessage(response, `No ${kind} data available`);
				slot.loading = false;
				slot.loaded = true;
				slot.missing = true;
				slot.error = slot.data === null ? null : message;
				return;
			}

			if (!response.ok) {
				throw new Error(await readErrorMessage(response, `Failed to load ${kind} data`));
			}

			const result = (await response.json()) as ResourceValueMap[K];
			if (token !== slot.requestToken) {
				return;
			}

			slot.data = result;
			slot.loading = false;
			slot.error = null;
			slot.loaded = true;
			slot.missing = false;
		} catch (error) {
			if (token !== slot.requestToken) {
				return;
			}

			slot.loading = false;
			slot.loaded = true;
			slot.error = error instanceof Error ? error.message : 'Unknown error occurred';
		}
	}

	private buildResourceUrl(kind: ResourceKind, router: string, side: Side) {
		return `/api/netflow/files/${this.slug}/${kind === 'ipCounts' ? 'ip-counts' : kind}?${new URLSearchParams(
			{
				dataset: this.dataset,
				...buildScopeSearchParams(this.flowScope),
				router,
				source: String(side === 'source')
			}
		).toString()}`;
	}
}

const fileDetailLoaders = new SvelteMap<string, NetflowFileDetailLoader>();

function evictStaleLoaders() {
	if (fileDetailLoaders.size <= FILE_DETAIL_CACHE_LIMIT) {
		return;
	}

	const entries = [...fileDetailLoaders.entries()].sort(
		([, left], [, right]) => left.lastAccessedAt - right.lastAccessedAt
	);

	while (fileDetailLoaders.size > FILE_DETAIL_CACHE_LIMIT) {
		const next = entries.shift();
		if (!next) {
			return;
		}

		fileDetailLoaders.delete(next[0]);
	}
}

export function getNetflowFileDetailLoader(dataset: string, slug: string, flowScope: FlowScope) {
	const key = buildLoaderKey(dataset, slug, flowScope);
	let loader = fileDetailLoaders.get(key);

	if (!loader) {
		loader = new NetflowFileDetailLoader(dataset, slug, flowScope);
		fileDetailLoaders.set(key, loader);
	}

	loader.touch();
	evictStaleLoaders();
	return loader;
}
