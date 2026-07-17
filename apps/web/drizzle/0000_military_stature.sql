CREATE TABLE `address_count_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_visibility` text NOT NULL,
	`dst_visibility` text NOT NULL,
	`address_side` text NOT NULL,
	`unique_address_count` integer NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_visibility`, `dst_visibility`, `address_side`),
	CONSTRAINT "address_count_stats_ip_version_check" CHECK("address_count_stats"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_address_count_stats_query` ON `address_count_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_visibility`,`dst_visibility`,`address_side`);--> statement-breakpoint
CREATE TABLE `address_structure_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_visibility` text NOT NULL,
	`dst_visibility` text NOT NULL,
	`address_side` text NOT NULL,
	`structure_kind` text NOT NULL,
	`values_json` text NOT NULL,
	`metadata_json` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_visibility`, `dst_visibility`, `address_side`, `structure_kind`),
	CONSTRAINT "address_structure_stats_ip_version_check" CHECK("address_structure_stats"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_address_structure_stats_query` ON `address_structure_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_visibility`,`dst_visibility`,`address_side`,`structure_kind`);--> statement-breakpoint
CREATE TABLE `datasets` (
	`id` text PRIMARY KEY NOT NULL,
	`label` text NOT NULL,
	`default_start_date` text NOT NULL,
	`source_mode` text DEFAULT 'static' NOT NULL,
	`discovery_mode` text DEFAULT 'static' NOT NULL,
	`sort_order` integer DEFAULT 0 NOT NULL
);
--> statement-breakpoint
CREATE TABLE `port_count_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_visibility` text NOT NULL,
	`dst_visibility` text NOT NULL,
	`port_side` text NOT NULL,
	`port_range` text NOT NULL,
	`unique_port_count` integer NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_visibility`, `dst_visibility`, `port_side`, `port_range`),
	CONSTRAINT "port_count_stats_ip_version_check" CHECK("port_count_stats"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_port_count_stats_query` ON `port_count_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_visibility`,`dst_visibility`,`port_side`,`port_range`);--> statement-breakpoint
CREATE TABLE `processed_inputs` (
	`input_kind` text NOT NULL,
	`input_locator` text NOT NULL,
	`source_id` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`status` text DEFAULT 'pending' NOT NULL,
	`error_message` text,
	`discovered_at` text DEFAULT CURRENT_TIMESTAMP,
	`processed_at` text,
	PRIMARY KEY(`input_kind`, `input_locator`, `source_id`, `bucket_start`)
);
--> statement-breakpoint
CREATE INDEX `idx_processed_inputs_source_bucket` ON `processed_inputs` (`source_id`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `protocol_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_visibility` text NOT NULL,
	`dst_visibility` text NOT NULL,
	`unique_protocols_count` integer NOT NULL,
	`protocols_list` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_visibility`, `dst_visibility`),
	CONSTRAINT "protocol_stats_ip_version_check" CHECK("protocol_stats"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_protocol_stats_query` ON `protocol_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_visibility`,`dst_visibility`);--> statement-breakpoint
CREATE TABLE `source_members` (
	`dataset_id` text NOT NULL,
	`source_id` text NOT NULL,
	`member_id` text NOT NULL,
	PRIMARY KEY(`dataset_id`, `source_id`, `member_id`)
);
--> statement-breakpoint
CREATE TABLE `traffic_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_visibility` text NOT NULL,
	`dst_visibility` text NOT NULL,
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
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_visibility`, `dst_visibility`),
	CONSTRAINT "traffic_stats_ip_version_check" CHECK("traffic_stats"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_traffic_stats_query` ON `traffic_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_visibility`,`dst_visibility`);