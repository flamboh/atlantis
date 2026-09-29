PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_address_count_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`address_side` text NOT NULL,
	`unique_address_count` integer NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_locality`, `dst_locality`, `address_side`),
	CONSTRAINT "address_count_stats_ip_version_check" CHECK("__new_address_count_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "address_count_stats_src_locality_check" CHECK("__new_address_count_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "address_count_stats_dst_locality_check" CHECK("__new_address_count_stats"."dst_locality" IN ('all', 'internal', 'external'))
);
--> statement-breakpoint
INSERT INTO `__new_address_count_stats`("source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "address_side", "unique_address_count", "processed_at") SELECT "source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_visibility", "dst_visibility", "address_side", "unique_address_count", "processed_at" FROM `address_count_stats` WHERE "src_visibility" = 'all' AND "dst_visibility" = 'all';--> statement-breakpoint
DROP TABLE `address_count_stats`;--> statement-breakpoint
ALTER TABLE `__new_address_count_stats` RENAME TO `address_count_stats`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `idx_address_count_stats_query` ON `address_count_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_locality`,`dst_locality`,`address_side`);--> statement-breakpoint
CREATE INDEX `idx_address_count_stats_timeseries` ON `address_count_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `__new_address_structure_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`address_side` text NOT NULL,
	`structure_kind` text NOT NULL,
	`values_json` text NOT NULL,
	`metadata_json` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_locality`, `dst_locality`, `address_side`, `structure_kind`),
	CONSTRAINT "address_structure_stats_ip_version_check" CHECK("__new_address_structure_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "address_structure_stats_src_locality_check" CHECK("__new_address_structure_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "address_structure_stats_dst_locality_check" CHECK("__new_address_structure_stats"."dst_locality" IN ('all', 'internal', 'external'))
);
--> statement-breakpoint
INSERT INTO `__new_address_structure_stats`("source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "address_side", "structure_kind", "values_json", "metadata_json", "processed_at") SELECT "source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_visibility", "dst_visibility", "address_side", "structure_kind", "values_json", "metadata_json", "processed_at" FROM `address_structure_stats` WHERE "src_visibility" = 'all' AND "dst_visibility" = 'all';--> statement-breakpoint
DROP TABLE `address_structure_stats`;--> statement-breakpoint
ALTER TABLE `__new_address_structure_stats` RENAME TO `address_structure_stats`;--> statement-breakpoint
CREATE INDEX `idx_address_structure_stats_query` ON `address_structure_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_locality`,`dst_locality`,`address_side`,`structure_kind`);--> statement-breakpoint
CREATE INDEX `idx_address_structure_stats_timeseries` ON `address_structure_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`ip_version`,`structure_kind`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `__new_port_count_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`port_side` text NOT NULL,
	`port_range` text NOT NULL,
	`unique_port_count` integer NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_locality`, `dst_locality`, `port_side`, `port_range`),
	CONSTRAINT "port_count_stats_ip_version_check" CHECK("__new_port_count_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "port_count_stats_src_locality_check" CHECK("__new_port_count_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "port_count_stats_dst_locality_check" CHECK("__new_port_count_stats"."dst_locality" IN ('all', 'internal', 'external'))
);
--> statement-breakpoint
INSERT INTO `__new_port_count_stats`("source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "port_side", "port_range", "unique_port_count", "processed_at") SELECT "source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_visibility", "dst_visibility", "port_side", "port_range", "unique_port_count", "processed_at" FROM `port_count_stats` WHERE "src_visibility" = 'all' AND "dst_visibility" = 'all';--> statement-breakpoint
DROP TABLE `port_count_stats`;--> statement-breakpoint
ALTER TABLE `__new_port_count_stats` RENAME TO `port_count_stats`;--> statement-breakpoint
CREATE INDEX `idx_port_count_stats_timeseries` ON `port_count_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `__new_protocol_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`unique_protocols_count` integer NOT NULL,
	`protocols_list` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_locality`, `dst_locality`),
	CONSTRAINT "protocol_stats_ip_version_check" CHECK("__new_protocol_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "protocol_stats_src_locality_check" CHECK("__new_protocol_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "protocol_stats_dst_locality_check" CHECK("__new_protocol_stats"."dst_locality" IN ('all', 'internal', 'external'))
);
--> statement-breakpoint
INSERT INTO `__new_protocol_stats`("source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "unique_protocols_count", "protocols_list", "processed_at") SELECT "source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_visibility", "dst_visibility", "unique_protocols_count", "protocols_list", "processed_at" FROM `protocol_stats` WHERE "src_visibility" = 'all' AND "dst_visibility" = 'all';--> statement-breakpoint
DROP TABLE `protocol_stats`;--> statement-breakpoint
ALTER TABLE `__new_protocol_stats` RENAME TO `protocol_stats`;--> statement-breakpoint
CREATE INDEX `idx_protocol_stats_timeseries` ON `protocol_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `__new_traffic_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`flows` integer NOT NULL,
	`flows_tcp` integer NOT NULL,
	`flows_udp` integer NOT NULL,
	`flows_icmp` integer NOT NULL,
	`flows_other` integer NOT NULL,
	`packets` integer NOT NULL,
	`packets_tcp` integer NOT NULL,
	`packets_udp` integer NOT NULL,
	`packets_icmp` integer NOT NULL,
	`packets_other` integer NOT NULL,
	`bytes` integer NOT NULL,
	`bytes_tcp` integer NOT NULL,
	`bytes_udp` integer NOT NULL,
	`bytes_icmp` integer NOT NULL,
	`bytes_other` integer NOT NULL,
	`duration_sum_ms` integer NOT NULL,
	`duration_count` integer NOT NULL,
	`average_duration_ms` real,
	`min_ttl_sum` integer NOT NULL,
	`min_ttl_count` integer NOT NULL,
	`average_min_ttl` real,
	`max_ttl_sum` integer NOT NULL,
	`max_ttl_count` integer NOT NULL,
	`average_max_ttl` real,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_locality`, `dst_locality`),
	CONSTRAINT "traffic_stats_ip_version_check" CHECK("__new_traffic_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "traffic_stats_src_locality_check" CHECK("__new_traffic_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "traffic_stats_dst_locality_check" CHECK("__new_traffic_stats"."dst_locality" IN ('all', 'internal', 'external'))
);
--> statement-breakpoint
INSERT INTO `__new_traffic_stats`("source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "flows", "flows_tcp", "flows_udp", "flows_icmp", "flows_other", "packets", "packets_tcp", "packets_udp", "packets_icmp", "packets_other", "bytes", "bytes_tcp", "bytes_udp", "bytes_icmp", "bytes_other", "duration_sum_ms", "duration_count", "average_duration_ms", "min_ttl_sum", "min_ttl_count", "average_min_ttl", "max_ttl_sum", "max_ttl_count", "average_max_ttl", "processed_at") SELECT "source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_visibility", "dst_visibility", "flows", "flows_tcp", "flows_udp", "flows_icmp", "flows_other", "packets", "packets_tcp", "packets_udp", "packets_icmp", "packets_other", "bytes", "bytes_tcp", "bytes_udp", "bytes_icmp", "bytes_other", "duration_sum_ms", "duration_count", "average_duration_ms", "min_ttl_sum", "min_ttl_count", "average_min_ttl", "max_ttl_sum", "max_ttl_count", "average_max_ttl", "processed_at" FROM `traffic_stats` WHERE "src_visibility" = 'all' AND "dst_visibility" = 'all';--> statement-breakpoint
DROP TABLE `traffic_stats`;--> statement-breakpoint
ALTER TABLE `__new_traffic_stats` RENAME TO `traffic_stats`;--> statement-breakpoint
CREATE INDEX `idx_traffic_stats_query` ON `traffic_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_locality`,`dst_locality`);--> statement-breakpoint
CREATE INDEX `idx_traffic_stats_timeseries` ON `traffic_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`bucket_start`);