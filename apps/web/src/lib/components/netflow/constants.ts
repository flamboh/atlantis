import type { DataOption } from '#lib/components/netflow/types.ts';
import type { NetflowMetricField } from '#lib/types/types.ts';

export const NETFLOW_DATA_OPTION_FIELDS: NetflowMetricField[] = [
	'flows',
	'flowsTcp',
	'flowsUdp',
	'flowsIcmp',
	'flowsOther',
	'packets',
	'packetsTcp',
	'packetsUdp',
	'packetsIcmp',
	'packetsOther',
	'bytes',
	'bytesTcp',
	'bytesUdp',
	'bytesIcmp',
	'bytesOther'
];

export const DEFAULT_DATA_OPTIONS: DataOption[] = [
	{ label: 'Flows TCP', index: 1, checked: true },
	{ label: 'Flows UDP', index: 2, checked: true },
	{ label: 'Flows ICMP', index: 3, checked: true },
	{ label: 'Flows Other', index: 4, checked: true },
	{ label: 'Packets TCP', index: 6, checked: false },
	{ label: 'Packets UDP', index: 7, checked: false },
	{ label: 'Packets ICMP', index: 8, checked: false },
	{ label: 'Packets Other', index: 9, checked: false },
	{ label: 'Bytes TCP', index: 11, checked: false },
	{ label: 'Bytes UDP', index: 12, checked: false },
	{ label: 'Bytes ICMP', index: 13, checked: false },
	{ label: 'Bytes Other', index: 14, checked: false }
];
