PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_address_structure_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`address_side` text NOT NULL,
	`measure` text NOT NULL,
	`structure_kind` text NOT NULL,
	`values_json` text NOT NULL,
	`metadata_json` text NOT NULL,
	`processed_at` text DEFAULT CURRENT_TIMESTAMP,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`, `ip_version`, `src_locality`, `dst_locality`, `address_side`, `measure`, `structure_kind`),
	CONSTRAINT "address_structure_stats_ip_version_check" CHECK("__new_address_structure_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "address_structure_stats_src_locality_check" CHECK("__new_address_structure_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "address_structure_stats_dst_locality_check" CHECK("__new_address_structure_stats"."dst_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "address_structure_stats_measure_check" CHECK("__new_address_structure_stats"."measure" IN ('addresses', 'packets', 'bytes')),
	CONSTRAINT "address_structure_stats_measure_spectrum_check" CHECK("__new_address_structure_stats"."measure" = 'addresses' OR "__new_address_structure_stats"."structure_kind" <> 'spectrum')
);
--> statement-breakpoint
INSERT INTO `__new_address_structure_stats`("source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "address_side", "measure", "structure_kind", "values_json", "metadata_json", "processed_at") SELECT "source_id", "granularity", "bucket_start", "bucket_end", "ip_version", "src_locality", "dst_locality", "address_side", 'addresses', "structure_kind", "values_json", "metadata_json", "processed_at" FROM `address_structure_stats`;--> statement-breakpoint
DROP TABLE `address_structure_stats`;--> statement-breakpoint
ALTER TABLE `__new_address_structure_stats` RENAME TO `address_structure_stats`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `idx_address_structure_stats_query` ON `address_structure_stats` (`granularity`,`bucket_start`,`source_id`,`ip_version`,`src_locality`,`dst_locality`,`address_side`,`measure`,`structure_kind`);--> statement-breakpoint
CREATE INDEX `idx_address_structure_stats_timeseries` ON `address_structure_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`ip_version`,`measure`,`structure_kind`,`bucket_start`);