use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, BufRead, BufReader, Write},
    net::Ipv4Addr,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use netflow_db::{
    compare::{CompareOptions, compare_databases},
    export::{ExtractRequest, extract_window, validate_extract_plan},
    maad,
    operations::{
        UgrAssetKind, scrape_ugr16_urls, select_web_verification_window, verify_web_routes,
    },
    prepare::{PrepareOptions, prepare_archive},
    reducer::{self, VisibilitySelection},
    registry::DatasetRegistry,
    storage::{backup_database, promote_database},
    verify::{VerifyOptions, verify_database},
};

#[derive(Debug, Parser)]
#[command(name = "netflow-db", version, about = "ATLANTIS NetFlow data pipeline")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the ingestion and aggregation pipeline.
    Pipeline(PipelineArgs),
    /// Create a bounded SQLite and/or Parquet analysis artifact.
    ExtractWindow(ExtractArgs),
    /// Verify a database against the web application's query contract.
    Verify(VerifyArgs),
    /// Compare a Rust candidate with a historical database over a half-open window.
    Compare(CompareArgs),
    /// Create a consistent SQLite backup or promote a candidate database.
    SqliteMaintenance(MaintenanceArgs),
    /// Extract/segment an immutable archive into a canonical nfcapd tree.
    PrepareNfcapd(PrepareArgs),
    /// Scrape deterministic UGR16 asset URLs.
    ScrapeUgr16(ScrapeArgs),
    /// Verify a running web application against a built database.
    VerifyWebRoutes(WebVerifyArgs),
    /// Reduce fixed-contract nfdump CSV from stdin to canonical JSON.
    Reduce(ReduceArgs),
    /// Compute MAAD JSON from IPv4 addresses, one per line.
    Maad(MaadArgs),
    /// Print the persisted pipeline contract version.
    ContractVersion,
}

#[derive(Debug, Args)]
struct PipelineArgs {
    #[arg(long, conflicts_with = "dataset")]
    config: Option<PathBuf>,
    #[arg(long, requires = "start_date", conflicts_with = "config")]
    dataset: Option<String>,
    #[arg(long)]
    start_date: Option<String>,
    #[arg(long)]
    end_date: Option<String>,
    #[arg(long)]
    start_time: Option<String>,
    #[arg(long)]
    end_time: Option<String>,
    #[arg(long)]
    database_path: Option<PathBuf>,
    #[arg(long)]
    datasets: Option<PathBuf>,
    #[arg(long)]
    ip_prefix: Option<String>,
    #[arg(long, value_enum)]
    src_visibility: Option<VisibilityArg>,
    #[arg(long, value_enum)]
    dst_visibility: Option<VisibilityArg>,
    #[arg(long, default_value = "nfdump")]
    nfdump: String,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    no_maad: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum VisibilityArg {
    Literal,
    Anonymized,
}

impl VisibilityArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Anonymized => "anonymized",
        }
    }

    const fn reducer(self) -> reducer::Visibility {
        match self {
            Self::Literal => reducer::Visibility::Literal,
            Self::Anonymized => reducer::Visibility::Anonymized,
        }
    }
}

#[derive(Debug, Args)]
struct ExtractArgs {
    #[arg(long, default_value = "uoregon")]
    dataset: String,
    #[arg(long)]
    source_db: Option<PathBuf>,
    #[arg(long)]
    output_dir: Option<PathBuf>,
    #[arg(long, default_value = "2025-06-01")]
    start: String,
    #[arg(long = "end", default_value = "2026-06-01")]
    end_exclusive: String,
    #[arg(long)]
    timezone: Option<String>,
    #[arg(long)]
    source_id: Option<String>,
    #[arg(long = "granularity", value_delimiter = ',')]
    granularities: Option<Vec<String>>,
    #[arg(long = "output", value_enum, default_value = "sqlite")]
    outputs: Vec<OutputArg>,
    #[arg(long)]
    parquet_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 5_000)]
    batch_size: usize,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OutputArg {
    Sqlite,
    Parquet,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    db_path: PathBuf,
    #[arg(long)]
    source_id: Option<String>,
    #[arg(long)]
    dataset_id: Option<String>,
    #[arg(long)]
    require_data: bool,
    #[arg(long)]
    require_maad_data: bool,
    #[arg(long)]
    require_processed: bool,
    #[arg(long)]
    require_rollup_parity: bool,
    #[arg(long)]
    require_no_raw_ip: bool,
}

#[derive(Debug, Args)]
struct CompareArgs {
    candidate: PathBuf,
    reference: PathBuf,
    #[arg(long)]
    start: String,
    #[arg(long)]
    end: String,
    #[arg(long, default_value = "America/Los_Angeles")]
    timezone: String,
    #[arg(long, default_value_t = 1e-10)]
    maad_absolute_tolerance: f64,
}

#[derive(Debug, Args)]
struct MaintenanceArgs {
    source_path: PathBuf,
    target_path: PathBuf,
    #[arg(long)]
    backup_existing: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PrepareArgs {
    #[arg(long)]
    archive: PathBuf,
    #[arg(long, required_unless_present = "dataset", conflicts_with = "dataset")]
    dataset_root: Option<PathBuf>,
    #[arg(
        long,
        required_unless_present = "dataset_root",
        conflicts_with = "dataset_root"
    )]
    dataset: Option<String>,
    #[arg(long)]
    datasets: Option<PathBuf>,
    #[arg(long, default_value = "default")]
    source: String,
    #[arg(long, default_value = "nfdump")]
    nfdump: String,
    #[arg(long, default_value = "America/Los_Angeles")]
    timezone: String,
    #[arg(long)]
    max_buckets: Option<usize>,
}

#[derive(Debug, Args)]
struct ScrapeArgs {
    #[arg(long)]
    base_url: String,
    #[arg(long, value_enum)]
    kind: AssetKindArg,
    #[arg(long = "month")]
    months: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AssetKindArg {
    Csv,
    Nfcapd,
}

#[derive(Debug, Args)]
struct WebVerifyArgs {
    #[arg(long)]
    db_path: PathBuf,
    #[arg(long)]
    base_url: Option<String>,
    #[arg(long, default_value = "ugr16")]
    dataset: String,
    #[arg(long, default_value = "ugr16")]
    source_id: String,
}

#[derive(Debug, Args)]
struct ReduceArgs {
    #[arg(long)]
    version: bool,
    #[arg(long, default_value_t = reducer::CONTRACT_VERSION)]
    contract_version: u32,
    #[arg(long, default_value = reducer::INPUT_CONTRACT)]
    input_contract: String,
    #[arg(long, default_value = reducer::OUTPUT_CONTRACT)]
    output_contract: String,
    #[arg(long, value_enum)]
    src_visibility: Option<VisibilityArg>,
    #[arg(long, value_enum)]
    dst_visibility: Option<VisibilityArg>,
}

#[derive(Debug, Args)]
struct MaadArgs {
    /// Read addresses from this file instead of standard input.
    input: Option<PathBuf>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    match Cli::parse().command {
        Command::Pipeline(args) => run_pipeline(args)?,
        Command::ExtractWindow(args) => run_extract(args)?,
        Command::Verify(args) => run_verify(args)?,
        Command::Compare(args) => run_compare(args)?,
        Command::SqliteMaintenance(args) => {
            if let Some(backup) = args.backup_existing {
                promote_database(&args.source_path, &args.target_path, Some(&backup))?;
            } else {
                backup_database(&args.source_path, &args.target_path)?;
            }
            println!(
                "published SQLite database: {} -> {}",
                args.source_path.display(),
                args.target_path.display()
            );
        }
        Command::PrepareNfcapd(args) => run_prepare(args)?,
        Command::ScrapeUgr16(args) => run_scrape(args)?,
        Command::VerifyWebRoutes(args) => run_web_verify(args)?,
        Command::Reduce(args) => run_reduce(args)?,
        Command::Maad(args) => run_maad(args)?,
        Command::ContractVersion => println!("{}", netflow_db::PIPELINE_CONTRACT_VERSION),
    }
    Ok(())
}

fn run_pipeline(args: PipelineArgs) -> Result<()> {
    let selection = serde_json::json!({
        "ip_prefix": args.ip_prefix,
        "src_visibility": args.src_visibility.map(VisibilityArg::as_str),
        "dst_visibility": args.dst_visibility.map(VisibilityArg::as_str),
    });
    netflow_db::pipeline::run(netflow_db::pipeline::PipelineRequest {
        config_path: args.config,
        dataset_id: args.dataset,
        datasets_path: args.datasets,
        start_date: args.start_date,
        end_date: args.end_date,
        start_time: args.start_time,
        end_time: args.end_time,
        database_path: args.database_path,
        selection,
        nfdump: args.nfdump,
        force: args.force,
        run_maad: !args.no_maad,
    })?;
    Ok(())
}

fn run_extract(args: ExtractArgs) -> Result<()> {
    if args.batch_size == 0 {
        bail!("--batch-size must be positive");
    }
    let timezone = args
        .timezone
        .or_else(|| std::env::var("NETFLOW_TIMEZONE").ok())
        .unwrap_or_else(|| "America/Los_Angeles".to_owned());
    let source_db = match args.source_db {
        Some(path) => path,
        None => default_dataset(&args.dataset)?.db_path.clone(),
    };
    let output_dir_explicit = args.output_dir.is_some();
    let output_dir = args.output_dir.unwrap_or_else(|| {
        let mut window = format!(
            "{}_to_{}",
            slug_path_part(&args.start),
            slug_path_part(&args.end_exclusive)
        );
        if let Some(source_id) = &args.source_id {
            window.push_str("_source-");
            window.push_str(&slug_path_part(source_id));
        }
        PathBuf::from("data")
            .join(slug_path_part(&args.dataset))
            .join("extracts")
            .join(window)
    });
    let start_ts = parse_boundary(&args.start, &timezone)?;
    let end_exclusive_ts = parse_boundary(&args.end_exclusive, &timezone)?;
    if end_exclusive_ts <= start_ts {
        bail!("--end must be after --start");
    }
    let request = ExtractRequest {
        dataset_id: args.dataset,
        source_db,
        output_dir,
        output_dir_explicit,
        start_ts,
        end_exclusive_ts,
        start_input: args.start,
        end_exclusive_input: args.end_exclusive,
        timezone,
        source_id: args.source_id,
        granularities: args.granularities,
        write_sqlite: args.outputs.contains(&OutputArg::Sqlite),
        write_parquet: args.outputs.contains(&OutputArg::Parquet),
        parquet_dir: args.parquet_dir,
        batch_size: args.batch_size,
    };
    if args.dry_run {
        validate_extract_plan(&request)?;
        println!(
            "dataset={} source_db={} output_dir={} window={}..{} outputs={}",
            request.dataset_id,
            request.source_db.display(),
            request.output_dir.display(),
            request.start_input,
            request.end_exclusive_input,
            args.outputs
                .iter()
                .map(|output| match output {
                    OutputArg::Sqlite => "sqlite",
                    OutputArg::Parquet => "parquet",
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        return Ok(());
    }
    let result = extract_window(&request)?;
    println!("wrote manifest: {}", result.manifest_path.display());
    Ok(())
}

fn run_verify(args: VerifyArgs) -> Result<()> {
    let report = verify_database(
        &args.db_path,
        &VerifyOptions {
            source_id: args.source_id,
            dataset_id: args.dataset_id,
            require_data: args.require_data,
            require_maad_data: args.require_maad_data,
            require_processed: args.require_processed,
            require_rollup_parity: args.require_rollup_parity,
            require_no_raw_ip: args.require_no_raw_ip,
        },
    )?;
    println!(
        "OK {} source={} window={}..{}",
        report.database.display(),
        report.source_id,
        report.bucket_start,
        report.bucket_end
    );
    Ok(())
}

fn run_compare(args: CompareArgs) -> Result<()> {
    let report = compare_databases(&CompareOptions {
        candidate: args.candidate,
        reference: args.reference,
        start_ts: parse_boundary(&args.start, &args.timezone)?,
        end_exclusive_ts: parse_boundary(&args.end, &args.timezone)?,
        maad_absolute_tolerance: args.maad_absolute_tolerance,
    })?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.compatible {
        bail!("candidate is not compatible with the reference in the selected window");
    }
    Ok(())
}

fn run_prepare(args: PrepareArgs) -> Result<()> {
    let dataset_root = match args.dataset_root {
        Some(path) => path,
        None => {
            let dataset_id = args
                .dataset
                .as_deref()
                .context("--dataset or --dataset-root is required")?;
            load_registry(args.datasets.as_deref())?
                .get(dataset_id)?
                .root_path
                .clone()
        }
    };
    let stats = prepare_archive(&PrepareOptions {
        archive: args.archive,
        dataset_root,
        source_id: args.source,
        nfdump: args.nfdump,
        timezone: args.timezone,
        interval_seconds: 300,
        max_buckets: args.max_buckets,
    })?;
    println!(
        "members={} written={} skipped_existing={}",
        stats.members, stats.written, stats.skipped_existing
    );
    Ok(())
}

fn load_registry(path: Option<&std::path::Path>) -> Result<DatasetRegistry> {
    let repository_root = std::env::current_dir()?;
    Ok(match path {
        Some(path) => DatasetRegistry::load(path, &repository_root)?,
        None => DatasetRegistry::load_default(&repository_root)?,
    })
}

fn default_dataset(dataset_id: &str) -> Result<netflow_db::registry::Dataset> {
    Ok(load_registry(None)?.get(dataset_id)?.clone())
}

fn slug_path_part(value: &str) -> String {
    let slug = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    if slug.is_empty() { "all".into() } else { slug }
}

fn run_scrape(args: ScrapeArgs) -> Result<()> {
    let kind = match args.kind {
        AssetKindArg::Csv => UgrAssetKind::Csv,
        AssetKindArg::Nfcapd => UgrAssetKind::Nfcapd,
    };
    let months = args.months.into_iter().collect::<BTreeSet<_>>();
    for url in scrape_ugr16_urls(&args.base_url, kind, &months)? {
        println!("{url}");
    }
    Ok(())
}

fn run_web_verify(args: WebVerifyArgs) -> Result<()> {
    let window = select_web_verification_window(&args.db_path, &args.source_id)?;
    let candidates = args
        .base_url
        .or_else(|| std::env::var("WEB_BASE_URL").ok())
        .map_or_else(
            || {
                vec![
                    "http://localhost:5173".to_owned(),
                    "http://localhost:4173".to_owned(),
                ]
            },
            |base_url| vec![base_url],
        );
    let mut failures = Vec::new();
    for base_url in candidates {
        match verify_web_routes(&base_url, &args.dataset, &args.source_id, window) {
            Ok(()) => {
                println!(
                    "OK web routes {} dataset={} window={}..{}",
                    base_url, args.dataset, window.start, window.end
                );
                return Ok(());
            }
            Err(error) => failures.push(format!("{base_url}: {error}")),
        }
    }
    bail!("web route verification failed: {}", failures.join("; "))
}

fn run_reduce(args: ReduceArgs) -> Result<()> {
    if args.version {
        println!("{}", reducer::VERSION_LINE);
        return Ok(());
    }
    if args.contract_version != reducer::CONTRACT_VERSION
        || args.input_contract != reducer::INPUT_CONTRACT
        || args.output_contract != reducer::OUTPUT_CONTRACT
    {
        bail!(
            "unsupported reducer contract: version={} input={} output={}",
            args.contract_version,
            args.input_contract,
            args.output_contract
        );
    }
    let selection = VisibilitySelection {
        source: args.src_visibility.map(VisibilityArg::reducer),
        destination: args.dst_visibility.map(VisibilityArg::reducer),
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    reducer::reduce_to_json(stdin.lock(), selection, stdout.lock())?;
    Ok(())
}

fn run_maad(args: MaadArgs) -> Result<()> {
    let input: Box<dyn BufRead> = match args.input {
        Some(path) => Box::new(BufReader::new(
            File::open(&path).with_context(|| format!("unable to open {}", path.display()))?,
        )),
        None => Box::new(BufReader::new(io::stdin())),
    };
    let mut addresses = Vec::new();
    for line in input.lines() {
        let line = line?;
        let value = line.trim();
        if value.is_empty() {
            continue;
        }
        addresses.push(
            value
                .parse::<Ipv4Addr>()
                .with_context(|| format!("invalid IPv4 address {value:?}"))?,
        );
    }
    maad::write_json(&maad::compute(addresses), io::stdout().lock())?;
    io::stdout().flush()?;
    Ok(())
}

fn parse_boundary(raw: &str, timezone: &str) -> Result<i64> {
    use std::str::FromStr;

    if raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return raw.parse().context("invalid Unix timestamp boundary");
    }
    if let Ok(timestamp) = jiff::Timestamp::from_str(raw) {
        return Ok(timestamp.as_second());
    }
    let datetime = if let Ok(date) = jiff::civil::Date::from_str(raw) {
        date.at(0, 0, 0, 0)
    } else {
        jiff::civil::DateTime::from_str(raw).context("invalid date/time boundary")?
    };
    Ok(datetime
        .in_tz(timezone)
        .context("invalid date/time timezone")?
        .timestamp()
        .as_second())
}
