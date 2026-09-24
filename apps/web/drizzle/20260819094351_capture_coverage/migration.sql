CREATE TABLE `bucket_coverage` (
	`source_id` text NOT NULL,
	`granularity` text NOT NULL,
	`bucket_start` integer NOT NULL,
	`bucket_end` integer NOT NULL,
	`coverage_state` text NOT NULL,
	`observed_units` integer NOT NULL,
	`expected_units` integer NOT NULL,
	`rejected_units` integer NOT NULL,
	PRIMARY KEY(`source_id`, `granularity`, `bucket_start`),
	CONSTRAINT "bucket_coverage_interval_check" CHECK("bucket_coverage"."bucket_end" > "bucket_coverage"."bucket_start"),
	CONSTRAINT "bucket_coverage_expected_check" CHECK("bucket_coverage"."expected_units" > 0),
	CONSTRAINT "bucket_coverage_observed_check" CHECK("bucket_coverage"."observed_units" >= 0 AND "bucket_coverage"."observed_units" <= "bucket_coverage"."expected_units"),
	CONSTRAINT "bucket_coverage_rejected_check" CHECK("bucket_coverage"."rejected_units" >= 0 AND "bucket_coverage"."rejected_units" <= "bucket_coverage"."expected_units"),
	CONSTRAINT "bucket_coverage_state_check" CHECK(("bucket_coverage"."coverage_state" = 'complete' AND "bucket_coverage"."observed_units" = "bucket_coverage"."expected_units" AND "bucket_coverage"."rejected_units" = 0) OR ("bucket_coverage"."coverage_state" = 'unknown' AND "bucket_coverage"."observed_units" = 0 AND "bucket_coverage"."rejected_units" = 0) OR ("bucket_coverage"."coverage_state" = 'partial' AND NOT ("bucket_coverage"."observed_units" = "bucket_coverage"."expected_units" AND "bucket_coverage"."rejected_units" = 0) AND NOT ("bucket_coverage"."observed_units" = 0 AND "bucket_coverage"."rejected_units" = 0)))
);
--> statement-breakpoint
CREATE INDEX `idx_bucket_coverage_query` ON `bucket_coverage` (`granularity`,`bucket_start`,`source_id`);
