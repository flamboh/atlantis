DROP INDEX `idx_port_count_stats_query`;--> statement-breakpoint
CREATE INDEX `idx_port_count_stats_timeseries` ON `port_count_stats` (`source_id`,`granularity`,`src_visibility`,`dst_visibility`,`bucket_start`);--> statement-breakpoint
DROP INDEX `idx_protocol_stats_query`;--> statement-breakpoint
CREATE INDEX `idx_protocol_stats_timeseries` ON `protocol_stats` (`source_id`,`granularity`,`src_visibility`,`dst_visibility`,`bucket_start`);--> statement-breakpoint
CREATE INDEX `idx_address_count_stats_timeseries` ON `address_count_stats` (`source_id`,`granularity`,`src_visibility`,`dst_visibility`,`bucket_start`);--> statement-breakpoint
CREATE INDEX `idx_address_structure_stats_timeseries` ON `address_structure_stats` (`source_id`,`granularity`,`src_visibility`,`dst_visibility`,`ip_version`,`structure_kind`,`bucket_start`);--> statement-breakpoint
CREATE INDEX `idx_traffic_stats_timeseries` ON `traffic_stats` (`source_id`,`granularity`,`src_visibility`,`dst_visibility`,`bucket_start`);--> statement-breakpoint
PRAGMA optimize;
