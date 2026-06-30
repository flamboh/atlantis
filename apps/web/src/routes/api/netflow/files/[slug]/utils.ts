import { createDateFromPSTComponents } from '$lib/utils/timezone';
import { getDatasetDb, getRequestedDataset } from '$lib/server/datasets';

export interface NetflowRecord {
	router: string;
	input_locator: string;
}

export function getDb(datasetOrPlatform?: string | App.Platform, platform?: App.Platform) {
	return getDatasetDb(datasetOrPlatform, platform);
}

export function slugToBucketStart(slug: string): number | null {
	if (slug.length !== 12) {
		return null;
	}

	const year = Number(slug.slice(0, 4));
	const month = Number(slug.slice(4, 6));
	const day = Number(slug.slice(6, 8));
	const hour = Number(slug.slice(8, 10));
	const minute = Number(slug.slice(10, 12));

	if (
		Number.isNaN(year) ||
		Number.isNaN(month) ||
		Number.isNaN(day) ||
		Number.isNaN(hour) ||
		Number.isNaN(minute)
	) {
		return null;
	}

	const date = createDateFromPSTComponents(year, month, day, hour, minute);
	return Math.floor(date.getTime() / 1000);
}

export async function getNetflowFilePath(
	platform: App.Platform | undefined,
	slug: string,
	router: string
): Promise<string | null> {
	const bucketStart = slugToBucketStart(slug);
	if (bucketStart === null) {
		return null;
	}

	const query = `
		SELECT input_locator FROM processed_inputs
		WHERE source_id = ? AND bucket_start = ? AND input_kind = 'nfcapd'
		LIMIT 1
	`;

	const database = await getDb(platform);
	const result = await database.get<NetflowRecord>(query, [router, bucketStart]);

	return result?.input_locator || null;
}

export function getDatasetFromRequest(url: URL, platform?: App.Platform): Promise<string> {
	return getRequestedDataset(url, platform);
}
