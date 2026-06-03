CREATE TABLE `datasets` (
	`id` text PRIMARY KEY NOT NULL,
	`label` text NOT NULL,
	`default_start_date` text NOT NULL,
	`source_mode` text DEFAULT 'static' NOT NULL,
	`discovery_mode` text DEFAULT 'static' NOT NULL,
	`sort_order` integer DEFAULT 0 NOT NULL
);
--> statement-breakpoint
INSERT INTO `datasets` (
	`id`,
	`label`,
	`default_start_date`,
	`source_mode`,
	`discovery_mode`,
	`sort_order`
) VALUES ('ugr16', 'UGR16', '2016-04-04', 'static', 'static', 0);
--> statement-breakpoint
CREATE TABLE `dimension_stats_v2` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`dimensions_json_sa` text NOT NULL,
	`dimensions_json_da` text NOT NULL,
	`metadata_json_sa` text NOT NULL,
	`metadata_json_da` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`),
	CONSTRAINT "dimension_stats_v2_ip_version_check" CHECK("dimension_stats_v2"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_dimension_stats_v2_granularity_bucket_source` ON `dimension_stats_v2` (`granularity`,`bucket_start`,`source_id`,`ip_version`);--> statement-breakpoint
CREATE TABLE `ip_stats_v2` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`sa_ipv4_count` integer NOT NULL,
	`da_ipv4_count` integer NOT NULL,
	`sa_ipv6_count` integer NOT NULL,
	`da_ipv6_count` integer NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`)
);
--> statement-breakpoint
CREATE INDEX `idx_ip_stats_v2_granularity_bucket_source` ON `ip_stats_v2` (`granularity`,`bucket_start`,`source_id`);--> statement-breakpoint
CREATE TABLE `netflow_stats_v2` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
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
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`),
	CONSTRAINT "netflow_stats_v2_ip_version_check" CHECK("netflow_stats_v2"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_netflow_stats_v2_granularity_bucket_source` ON `netflow_stats_v2` (`granularity`,`bucket_start`,`source_id`,`ip_version`);--> statement-breakpoint
CREATE TABLE `processed_inputs_v2` (
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
CREATE INDEX `idx_processed_inputs_v2_source_bucket` ON `processed_inputs_v2` (`source_id`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `protocol_stats_v2` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`unique_protocols_count_ipv4` integer NOT NULL,
	`unique_protocols_count_ipv6` integer NOT NULL,
	`protocols_list_ipv4` text NOT NULL,
	`protocols_list_ipv6` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`)
);
--> statement-breakpoint
CREATE INDEX `idx_protocol_stats_v2_granularity_bucket_source` ON `protocol_stats_v2` (`granularity`,`bucket_start`,`source_id`);--> statement-breakpoint
CREATE TABLE `spectrum_stats_v2` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`spectrum_json_sa` text NOT NULL,
	`spectrum_json_da` text NOT NULL,
	`metadata_json_sa` text NOT NULL,
	`metadata_json_da` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`),
	CONSTRAINT "spectrum_stats_v2_ip_version_check" CHECK("spectrum_stats_v2"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_spectrum_stats_v2_granularity_bucket_source` ON `spectrum_stats_v2` (`granularity`,`bucket_start`,`source_id`,`ip_version`);--> statement-breakpoint
CREATE TABLE `structure_stats_v2` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`structure_json_sa` text NOT NULL,
	`structure_json_da` text NOT NULL,
	`metadata_json_sa` text NOT NULL,
	`metadata_json_da` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`),
	CONSTRAINT "structure_stats_v2_ip_version_check" CHECK("structure_stats_v2"."ip_version" IN (4, 6))
);
--> statement-breakpoint
CREATE INDEX `idx_structure_stats_v2_granularity_bucket_source` ON `structure_stats_v2` (`granularity`,`bucket_start`,`source_id`,`ip_version`);
