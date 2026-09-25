CREATE TABLE `address_maad_stats` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`ip_version` integer NOT NULL,
	`src_locality` text NOT NULL,
	`dst_locality` text NOT NULL,
	`address_side` text NOT NULL,
	`measure` text NOT NULL,
	`total_addrs` integer NOT NULL,
	`zero_weight_addrs` integer DEFAULT 0 NOT NULL,
	`min_prefix_length` integer,
	`max_prefix_length` integer,
	`d0` real,
	`d1` real,
	`d2` real,
	`tau` blob,
	`tau_sd` blob,
	`spectrum` blob,
	CONSTRAINT "address_maad_stats_ip_version_check" CHECK("address_maad_stats"."ip_version" IN (4, 6)),
	CONSTRAINT "address_maad_stats_src_locality_check" CHECK("address_maad_stats"."src_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "address_maad_stats_dst_locality_check" CHECK("address_maad_stats"."dst_locality" IN ('all', 'internal', 'external')),
	CONSTRAINT "address_maad_stats_measure_check" CHECK("address_maad_stats"."measure" IN ('addresses', 'packets', 'bytes')),
	CONSTRAINT "address_maad_stats_bucket_check" CHECK("address_maad_stats"."bucket_end" > "address_maad_stats"."bucket_start"),
	CONSTRAINT "address_maad_stats_total_addrs_check" CHECK("address_maad_stats"."total_addrs" >= 0),
	CONSTRAINT "address_maad_stats_zero_weight_addrs_check" CHECK("address_maad_stats"."zero_weight_addrs" >= 0 AND ("address_maad_stats"."measure" <> 'addresses' OR "address_maad_stats"."zero_weight_addrs" = 0)),
	CONSTRAINT "address_maad_stats_prefix_length_check" CHECK("address_maad_stats"."max_prefix_length" >= "address_maad_stats"."min_prefix_length"),
	CONSTRAINT "address_maad_stats_tau_sd_check" CHECK(length("address_maad_stats"."tau_sd") IS length("address_maad_stats"."tau")),
	CONSTRAINT "address_maad_stats_spectrum_check" CHECK(length("address_maad_stats"."spectrum") % 8 = 0),
	CONSTRAINT "address_maad_stats_measure_spectrum_check" CHECK("address_maad_stats"."measure" = 'addresses' OR "address_maad_stats"."spectrum" IS NULL),
	CONSTRAINT "address_maad_stats_tau_d0_check" CHECK(("address_maad_stats"."tau" IS NULL) = ("address_maad_stats"."d0" IS NULL))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `idx_address_maad_stats_key` ON `address_maad_stats` (`source_id`,`granularity`,`src_locality`,`dst_locality`,`ip_version`,`measure`,`bucket_start`,`address_side`);--> statement-breakpoint
CREATE INDEX `idx_address_maad_stats_bucket` ON `address_maad_stats` (`granularity`,`bucket_start`);--> statement-breakpoint
CREATE TABLE `maad_q_grid` (
	`ip_version` integer PRIMARY KEY NOT NULL,
	`q_min` real NOT NULL,
	`q_step` real NOT NULL,
	`q_count` integer NOT NULL,
	CONSTRAINT "maad_q_grid_ip_version_check" CHECK("maad_q_grid"."ip_version" IN (4, 6)),
	CONSTRAINT "maad_q_grid_q_step_check" CHECK("maad_q_grid"."q_step" > 0),
	CONSTRAINT "maad_q_grid_q_count_check" CHECK("maad_q_grid"."q_count" > 0)
);
--> statement-breakpoint
DROP TABLE `address_structure_stats`;