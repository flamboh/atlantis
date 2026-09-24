import type { AlertSort, AlertsFeedResponse, AlertTail } from '#lib/types/types.ts';

export type QueryParam = string | number | boolean | null | Uint8Array;

export type PreparedStatement = {
	get<T = unknown>(...params: QueryParam[]): Promise<T | undefined>;
	all<T = unknown>(...params: QueryParam[]): Promise<T[]>;
};

export interface ReadonlyDatasetDb {
	get<T = unknown>(query: string, params?: QueryParam[]): Promise<T | undefined>;
	all<T = unknown>(query: string, params?: QueryParam[]): Promise<T[]>;
	prepare(sql: string): PreparedStatement;
}

export type DatasetRow = {
	id: string;
	label: string;
	defaultStartDate: string;
	discoveryMode: string;
	sortOrder: number;
};

export type SourceDefinition = {
	sourceId: string;
	members: string[];
};

export type SourceMetadata = {
	sourceIds?: string[];
	definitions?: SourceDefinition[];
};

export type DatasetDbLease = {
	db: ReadonlyDatasetDb;
	sourceMetadata?: Map<string, SourceMetadata>;
	release(): void;
};

export type AlertsFeedQuery = {
	horizonSeconds: number;
	tail?: AlertTail;
	sort?: AlertSort;
	limit?: number;
};

export type DatabaseDriver = {
	listDatasetRows(): Promise<DatasetRow[]>;
	getDatasetRow(datasetId: string): Promise<DatasetRow>;
	acquireDatasetDb(datasetId: string): Promise<DatasetDbLease>;
	readAlertsFeed(datasetId: string, query: AlertsFeedQuery): Promise<AlertsFeedResponse | null>;
};

export function makePrepared(db: ReadonlyDatasetDb, query: string): PreparedStatement {
	return {
		get: <T = unknown>(...params: QueryParam[]) => db.get<T>(query, params),
		all: <T = unknown>(...params: QueryParam[]) => db.all<T>(query, params)
	};
}
