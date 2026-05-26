import { json } from '@sveltejs/kit';
import type { RequestHandler } from './$types';
import type {
	NetflowIpFamily,
	NetflowMetricField,
	NetflowSplitMetricField,
	NetflowStatsResponse,
	NetflowStatsResult
} from '$lib/types/types';
import {
	getDatasetDb,
	getRequestedDataset,
	listDatasetSourceDefinitions
} from '$lib/server/datasets';
import { NETFLOW_DATA_OPTION_FIELDS } from '$lib/components/netflow/constants';
import {
	getBucketStartQuery,
	groupByToGranularity,
	parseSourceIds,
	parseTimestamp,
	placeholders,
	resolveSourceIds
} from '$lib/server/netflow-v2';

const V2_IP_VERSION_BY_FAMILY: Record<Exclude<NetflowIpFamily, 'all'>, 4 | 6> = {
	ipv4: 4,
	ipv6: 6
};

function getBaseMetricSelects(): string[] {
	return NETFLOW_DATA_OPTION_FIELDS.map((field) => {
		const columnName = field.replace(/[A-Z]/g, (match) => `_${match.toLowerCase()}`).toLowerCase();
		return `SUM(${columnName}) AS ${field}`;
	});
}

function getFamilyMetricSelects(family: Exclude<NetflowIpFamily, 'all'>): string[] {
	const ipVersion = V2_IP_VERSION_BY_FAMILY[family];
	const suffix = family === 'ipv4' ? 'Ipv4' : 'Ipv6';
	return NETFLOW_DATA_OPTION_FIELDS.map((field) => {
		const columnName = field.replace(/[A-Z]/g, (match) => `_${match.toLowerCase()}`).toLowerCase();
		return `SUM(CASE WHEN ip_version = ${ipVersion} THEN ${columnName} ELSE 0 END) AS ${field}${suffix}`;
	});
}

function getMetricValue(
	row: Record<string, number | null>,
	field: NetflowMetricField | NetflowSplitMetricField
): number {
	return row[field] ?? 0;
}

function normalizeRow(row: Record<string, number | null>): NetflowStatsResult {
	return {
		bucketStart: row.bucketStart ?? 0,
		flows: getMetricValue(row, 'flows'),
		flowsTcp: getMetricValue(row, 'flowsTcp'),
		flowsUdp: getMetricValue(row, 'flowsUdp'),
		flowsIcmp: getMetricValue(row, 'flowsIcmp'),
		flowsOther: getMetricValue(row, 'flowsOther'),
		packets: getMetricValue(row, 'packets'),
		packetsTcp: getMetricValue(row, 'packetsTcp'),
		packetsUdp: getMetricValue(row, 'packetsUdp'),
		packetsIcmp: getMetricValue(row, 'packetsIcmp'),
		packetsOther: getMetricValue(row, 'packetsOther'),
		bytes: getMetricValue(row, 'bytes'),
		bytesTcp: getMetricValue(row, 'bytesTcp'),
		bytesUdp: getMetricValue(row, 'bytesUdp'),
		bytesIcmp: getMetricValue(row, 'bytesIcmp'),
		bytesOther: getMetricValue(row, 'bytesOther'),
		flowsIpv4: getMetricValue(row, 'flowsIpv4'),
		flowsTcpIpv4: getMetricValue(row, 'flowsTcpIpv4'),
		flowsUdpIpv4: getMetricValue(row, 'flowsUdpIpv4'),
		flowsIcmpIpv4: getMetricValue(row, 'flowsIcmpIpv4'),
		flowsOtherIpv4: getMetricValue(row, 'flowsOtherIpv4'),
		packetsIpv4: getMetricValue(row, 'packetsIpv4'),
		packetsTcpIpv4: getMetricValue(row, 'packetsTcpIpv4'),
		packetsUdpIpv4: getMetricValue(row, 'packetsUdpIpv4'),
		packetsIcmpIpv4: getMetricValue(row, 'packetsIcmpIpv4'),
		packetsOtherIpv4: getMetricValue(row, 'packetsOtherIpv4'),
		bytesIpv4: getMetricValue(row, 'bytesIpv4'),
		bytesTcpIpv4: getMetricValue(row, 'bytesTcpIpv4'),
		bytesUdpIpv4: getMetricValue(row, 'bytesUdpIpv4'),
		bytesIcmpIpv4: getMetricValue(row, 'bytesIcmpIpv4'),
		bytesOtherIpv4: getMetricValue(row, 'bytesOtherIpv4'),
		flowsIpv6: getMetricValue(row, 'flowsIpv6'),
		flowsTcpIpv6: getMetricValue(row, 'flowsTcpIpv6'),
		flowsUdpIpv6: getMetricValue(row, 'flowsUdpIpv6'),
		flowsIcmpIpv6: getMetricValue(row, 'flowsIcmpIpv6'),
		flowsOtherIpv6: getMetricValue(row, 'flowsOtherIpv6'),
		packetsIpv6: getMetricValue(row, 'packetsIpv6'),
		packetsTcpIpv6: getMetricValue(row, 'packetsTcpIpv6'),
		packetsUdpIpv6: getMetricValue(row, 'packetsUdpIpv6'),
		packetsIcmpIpv6: getMetricValue(row, 'packetsIcmpIpv6'),
		packetsOtherIpv6: getMetricValue(row, 'packetsOtherIpv6'),
		bytesIpv6: getMetricValue(row, 'bytesIpv6'),
		bytesTcpIpv6: getMetricValue(row, 'bytesTcpIpv6'),
		bytesUdpIpv6: getMetricValue(row, 'bytesUdpIpv6'),
		bytesIcmpIpv6: getMetricValue(row, 'bytesIcmpIpv6'),
		bytesOtherIpv6: getMetricValue(row, 'bytesOtherIpv6')
	};
}

export const GET: RequestHandler = async ({ url, platform }) => {
	const dataset = await getRequestedDataset(url, platform);
	const startDate = url.searchParams.get('startDate') || '';
	const endDate = url.searchParams.get('endDate') || '';
	const groupBy = url.searchParams.get('groupBy') || 'date';
	const routers = parseSourceIds(url.searchParams.get('routers'));
	const start = parseTimestamp(startDate);
	const end = parseTimestamp(endDate);

	if (routers.length === 0) {
		return json({ error: 'No routers selected' }, { status: 400 });
	}

	if (start === null || end === null) {
		return json({ error: 'Invalid start or end time' }, { status: 400 });
	}

	if (start >= end) {
		return json({ error: 'Start time must be before end time' }, { status: 400 });
	}

	try {
		const db = await getDatasetDb(dataset, platform);
		const resolvedSources = resolveSourceIds(
			await listDatasetSourceDefinitions(dataset, platform),
			routers,
			'additive'
		);
		const granularity = groupByToGranularity(groupBy);
		const useAggregate = granularity !== '5m';
		const timeColumn = 'bucket_start';
		const sourceColumn = 'source_id';
		const tableName = useAggregate ? 'netflow_stats_aggregate_v2' : 'netflow_stats_v2';
		const bucketStartQuery = useAggregate ? timeColumn : getBucketStartQuery(timeColumn, groupBy);
		const metricSelects = getBaseMetricSelects();
		metricSelects.push(...getFamilyMetricSelects('ipv4'), ...getFamilyMetricSelects('ipv6'));
		const granularityClause = useAggregate ? 'AND granularity = ?' : '';

		const query = `
			SELECT 
				${bucketStartQuery} as bucketStart,
				${metricSelects.join(',\n\t\t\t\t')}
			FROM ${tableName} 
			WHERE ${sourceColumn} IN (${placeholders(resolvedSources)})
			${granularityClause}
			AND ${timeColumn} >= ? 
			AND ${timeColumn} < ?
			GROUP BY bucketStart
			ORDER BY bucketStart
		`;

		const params = useAggregate
			? [...resolvedSources, granularity, start, end]
			: [...resolvedSources, start, end];

		const rows = await db.all<Record<string, number | null>>(query, params);
		const result = rows.map(normalizeRow);
		const availableIpFamilies: NetflowIpFamily[] = ['all', 'ipv4', 'ipv6'];
		return json({ result, availableIpFamilies } satisfies NetflowStatsResponse);
	} catch (error) {
		console.error('Database error:', error);
		return json({ error: 'Database query failed' }, { status: 500 });
	}
};
