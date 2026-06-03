import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type {
	FileIpCounts,
	NetflowFileDetailsResponse,
	NetflowFileDetailsRouter,
	NetflowFileSummaryRecord,
	SpectrumData,
	SpectrumPoint,
	StructureFunctionData,
	StructureFunctionPoint
} from '$lib/types/types';
import { getDatasetFromRequest, getDb, slugToBucketStart } from '../utils';
import { normalizeStructurePoints } from '$lib/server/netflow-v2';

const FIVE_MINUTES = '5m';

type FileDetailsRow = NetflowFileSummaryRecord & {
	input_kind: string | null;
	input_status: string | null;
	input_error_message: string | null;
	saIpv4Count: number | null;
	daIpv4Count: number | null;
	saIpv6Count: number | null;
	daIpv6Count: number | null;
	structureJsonSa: string | null;
	structureJsonDa: string | null;
	spectrumJsonSa: string | null;
	spectrumJsonDa: string | null;
};

function parseStructure(raw: string | null): StructureFunctionPoint[] | null {
	if (!raw) return null;

	try {
		return normalizeStructurePoints(JSON.parse(raw) as StructureFunctionPoint[]);
	} catch (error) {
		console.error('Failed to parse structure JSON from database:', error);
		return null;
	}
}

function parseSpectrum(raw: string | null): SpectrumPoint[] | null {
	if (!raw) return null;

	try {
		return JSON.parse(raw) as SpectrumPoint[];
	} catch (error) {
		console.error('Failed to parse spectrum JSON from database:', error);
		return null;
	}
}

function buildStructureData(
	slug: string,
	router: string,
	addressType: 'Source' | 'Destination',
	points: StructureFunctionPoint[] | null
): StructureFunctionData | null {
	if (!points || points.length === 0) return null;

	const qValues = points.map((point) => point.q);
	const qRange =
		qValues.length > 0
			? { min: Math.min(...qValues), max: Math.max(...qValues) }
			: { min: 0, max: 0 };

	return {
		slug,
		router,
		filename: `nfcapd.${slug}`,
		structureFunction: points,
		metadata: {
			dataSource: `Database: structure_stats 5m bucket (${addressType} Addresses)`,
			uniqueIPCount: -1,
			pointCount: points.length,
			addressType,
			qRange
		}
	};
}

function buildSpectrumData(
	slug: string,
	router: string,
	addressType: 'Source' | 'Destination',
	points: SpectrumPoint[] | null
): SpectrumData | null {
	if (!points || points.length === 0) return null;

	const alphaValues = points.map((point) => point.alpha);
	const alphaRange =
		alphaValues.length > 0
			? { min: Math.min(...alphaValues), max: Math.max(...alphaValues) }
			: { min: 0, max: 0 };

	return {
		slug,
		router,
		filename: `nfcapd.${slug}`,
		spectrum: points,
		metadata: {
			dataSource: `Database: spectrum_stats 5m bucket (${addressType} Addresses)`,
			uniqueIPCount: -1,
			pointCount: points.length,
			addressType,
			alphaRange
		}
	};
}

function buildIpCounts(ipv4Count: number | null, ipv6Count: number | null): FileIpCounts | null {
	if (ipv4Count === null && ipv6Count === null) {
		return null;
	}

	return {
		ipv4Count,
		ipv6Count
	};
}

export const GET: RequestHandler = async ({ params, url, platform }) => {
	const { slug } = params;
	const dataset = await getDatasetFromRequest(url, platform);

	if (!slug || slug.length !== 12 || !/^\d{12}$/.test(slug)) {
		return json({ error: 'Invalid slug format' }, { status: 400 });
	}

	const bucketStart = slugToBucketStart(slug);
	if (bucketStart === null) {
		return json({ error: 'Unable to parse slug timestamp' }, { status: 400 });
	}

	try {
		const db = await getDb(dataset, platform);
		const rows = await db.all<FileDetailsRow>(
			`SELECT
				ns.*,
				pi.input_locator AS file_path,
				pi.input_kind,
				pi.status AS input_status,
				pi.error_message AS input_error_message,
				COALESCE(ns.processed_at, pi.processed_at, pi.discovered_at) AS processed_at,
				ip.sa_ipv4_count AS saIpv4Count,
				ip.da_ipv4_count AS daIpv4Count,
				ip.sa_ipv6_count AS saIpv6Count,
				ip.da_ipv6_count AS daIpv6Count,
				st.structure_json_sa AS structureJsonSa,
				st.structure_json_da AS structureJsonDa,
				sp.spectrum_json_sa AS spectrumJsonSa,
				sp.spectrum_json_da AS spectrumJsonDa
			FROM (
				SELECT
					source_id AS router,
					bucket_start,
					MAX(bucket_end) AS bucket_end,
					SUM(flows) AS flows,
					SUM(flows_tcp) AS flows_tcp,
					SUM(flows_udp) AS flows_udp,
					SUM(flows_icmp) AS flows_icmp,
					SUM(flows_other) AS flows_other,
					SUM(packets) AS packets,
					SUM(packets_tcp) AS packets_tcp,
					SUM(packets_udp) AS packets_udp,
					SUM(packets_icmp) AS packets_icmp,
					SUM(packets_other) AS packets_other,
					SUM(bytes) AS bytes,
					SUM(bytes_tcp) AS bytes_tcp,
					SUM(bytes_udp) AS bytes_udp,
					SUM(bytes_icmp) AS bytes_icmp,
					SUM(bytes_other) AS bytes_other,
					NULL AS first_timestamp,
					NULL AS last_timestamp,
					NULL AS msec_first,
					NULL AS msec_last,
					NULL AS sequence_failures,
					MAX(processed_at) AS processed_at
				FROM netflow_stats_v2
				WHERE granularity = ?
					AND bucket_start = ?
				GROUP BY source_id, bucket_start
			) ns
			LEFT JOIN (
				SELECT
					source_id,
					bucket_start,
					MIN(input_locator) AS input_locator,
					MIN(input_kind) AS input_kind,
					MAX(status) AS status,
					MAX(error_message) AS error_message,
					MAX(discovered_at) AS discovered_at,
					MAX(processed_at) AS processed_at
				FROM processed_inputs_v2
				WHERE bucket_start = ?
				GROUP BY source_id, bucket_start
			) pi
				ON pi.source_id = ns.router
				AND pi.bucket_start = ns.bucket_start
			LEFT JOIN ip_stats_v2 ip
				ON ip.source_id = ns.router
				AND ip.granularity = ?
				AND ip.bucket_start = ns.bucket_start
			LEFT JOIN structure_stats_v2 st
				ON st.source_id = ns.router
				AND st.granularity = ?
				AND st.bucket_start = ns.bucket_start
				AND st.ip_version = 4
			LEFT JOIN spectrum_stats_v2 sp
				ON sp.source_id = ns.router
				AND sp.granularity = ?
				AND sp.bucket_start = ns.bucket_start
				AND sp.ip_version = 4
			ORDER BY ns.router`,
			[FIVE_MINUTES, bucketStart, bucketStart, FIVE_MINUTES, FIVE_MINUTES, FIVE_MINUTES]
		);

		if (rows.length === 0) {
			return json({ error: `No data found for bucket: ${slug}` }, { status: 404 });
		}

		const routers: NetflowFileDetailsRouter[] = rows.map((row) => {
			const structureSourcePoints = parseStructure(row.structureJsonSa);
			const structureDestinationPoints = parseStructure(row.structureJsonDa);
			const spectrumSourcePoints = parseSpectrum(row.spectrumJsonSa);
			const spectrumDestinationPoints = parseSpectrum(row.spectrumJsonDa);

			return {
				summary: {
					router: row.router,
					file_path: row.file_path,
					file_exists_on_disk: false,
					input_kind: row.input_kind,
					input_status: row.input_status,
					input_error_message: row.input_error_message,
					bucket_start: row.bucket_start,
					bucket_end: row.bucket_end,
					flows: row.flows,
					flows_tcp: row.flows_tcp,
					flows_udp: row.flows_udp,
					flows_icmp: row.flows_icmp,
					flows_other: row.flows_other,
					packets: row.packets,
					packets_tcp: row.packets_tcp,
					packets_udp: row.packets_udp,
					packets_icmp: row.packets_icmp,
					packets_other: row.packets_other,
					bytes: row.bytes,
					bytes_tcp: row.bytes_tcp,
					bytes_udp: row.bytes_udp,
					bytes_icmp: row.bytes_icmp,
					bytes_other: row.bytes_other,
					first_timestamp: row.first_timestamp,
					last_timestamp: row.last_timestamp,
					msec_first: row.msec_first,
					msec_last: row.msec_last,
					sequence_failures: row.sequence_failures,
					processed_at: row.processed_at
				},
				ipCountsSource: buildIpCounts(row.saIpv4Count, row.saIpv6Count),
				ipCountsDestination: buildIpCounts(row.daIpv4Count, row.daIpv6Count),
				structureSource: buildStructureData(slug, row.router, 'Source', structureSourcePoints),
				structureDestination: buildStructureData(
					slug,
					row.router,
					'Destination',
					structureDestinationPoints
				),
				spectrumSource: buildSpectrumData(slug, row.router, 'Source', spectrumSourcePoints),
				spectrumDestination: buildSpectrumData(
					slug,
					row.router,
					'Destination',
					spectrumDestinationPoints
				)
			};
		});

		const response: NetflowFileDetailsResponse = {
			routers
		};

		return json(response);
	} catch (error) {
		console.error('Failed to fetch file details from database:', error);
		return json({ error: 'Failed to fetch file details' }, { status: 500 });
	}
};
