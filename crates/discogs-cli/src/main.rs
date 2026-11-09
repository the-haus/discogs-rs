//! The `discogs` command.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use discogs_core::generate::{generate, targets_for_hit_rate, GenerateOptions};
use discogs_core::remote::{Http, HttpOptions};
use discogs_core::{profile_path, run_profile, ProfileOptions, RecordKind, RunOptions, Summary, TargetSet};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "discogs", version, about = "Read the Discogs data dumps fast.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Per-artist genre and style counts from a releases or masters dump.
    Profile {
        /// The dump: a .xml.gz or .xml path, or an https:// URL (streamed,
        /// never stored unless --keep).
        #[arg(long)]
        input: String,
        /// Target Discogs artist ids, one per line.
        #[arg(long)]
        targets: PathBuf,
        /// The input's expected SHA-256, or `auto`: the sum the dump's
        /// CHECKSUM.txt gives for the input's file name. On a mismatch
        /// nothing is written.
        #[arg(long)]
        checksum: Option<String>,
        /// Skip-unchanged state (JSON): a dump already profiled with the
        /// same checksum, targets and options is not read again.
        #[arg(long)]
        state: Option<PathBuf>,
        /// URL input: also write the download here as it streams (renamed
        /// into place once the run succeeds).
        #[arg(long)]
        keep: Option<PathBuf>,
        /// URL input: extra whole tries after a dropped connection.
        #[arg(long, default_value_t = 2)]
        retries: u32,
        /// URL input: seconds to wait for a connection or the next bytes.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// Scanner threads (default: cores less one).
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// `releases` or `masters` (default: read from the file).
        #[arg(long)]
        kind: Option<RecordKind>,
        /// Block size in MB.
        #[arg(long, default_value_t = 16)]
        block_mb: usize,
        /// Output: .parquet or .jsonl.
        #[arg(long)]
        out: PathBuf,
        /// Skip the SHA-256 of the input (ignored with --checksum).
        #[arg(long)]
        no_hash: bool,
        /// Print the summary as JSON on stdout.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        site: Site,
    },
    /// Print a dump's CHECKSUM.txt: `<sha256>  <file>` per line.
    Checksum {
        /// The dump, e.g. 20261101.
        #[arg(long)]
        dump: String,
        /// Print a JSON object (file → sha256) instead.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        site: Site,
    },
    /// Print the newest complete dump (the newest whose CHECKSUM.txt is listed).
    Latest {
        #[command(flatten)]
        site: Site,
    },
    /// Generate a synthetic dump and time each stage on it.
    Bench {
        /// Gigabytes (10^9 bytes) of XML to generate.
        #[arg(long)]
        generate: f64,
        /// Where the synthetic files go (reused when present).
        #[arg(long, default_value = "bench-work")]
        dir: PathBuf,
        /// Hit rates to run, comma-separated.
        #[arg(long, default_value = "0.01,0.1", value_delimiter = ',')]
        hit_rates: Vec<f64>,
        /// The many-thread run's scanner threads (default: cores less one).
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// `releases` or `masters`.
        #[arg(long, default_value = "releases")]
        kind: RecordKind,
        /// Print the results as JSON on stdout.
        #[arg(long)]
        json: bool,
    },
}

/// Where the dumps are.
#[derive(clap::Args)]
struct Site {
    /// The Discogs data site.
    #[arg(long, env = "DISCOGS_DATA_URL", default_value = discogs_core::dumps::DISCOGS_DATA)]
    base_url: String,
}

impl Site {
    fn http(&self) -> Result<Http> {
        Ok(Http::new(HttpOptions {
            base_url: self.base_url.clone(),
            ..Default::default()
        })?)
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Profile {
            input,
            targets,
            checksum,
            state,
            keep,
            retries,
            timeout,
            threads,
            kind,
            block_mb,
            out,
            no_hash,
            json,
            site,
        } => {
            let text = std::fs::read(&targets).with_context(|| format!("{}", targets.display()))?;
            let targets = TargetSet::parse(&text)?;
            let options = RunOptions {
                source: input,
                out: out.clone(),
                checksum,
                state,
                keep,
                retries,
                base_url: Some(site.base_url),
                timeout: Some(Duration::from_secs(timeout)),
                profile: ProfileOptions {
                    threads,
                    block_size: block_mb << 20,
                    checksum: None,
                    kind,
                    no_hash,
                },
            };
            let run = run_profile(&targets, &options)?;
            if json {
                let mut v = serde_json::to_value(&run)?;
                v["seconds"]["write"] = run.write_seconds.into();
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else if run.skipped {
                eprintln!(
                    "{}: unchanged since the last run (sha256 {}, same targets and options): skipped; {} stays",
                    run.source,
                    run.profile.sha256,
                    out.display()
                );
            } else {
                print_summary(&run.profile, targets.len());
                if run.download_seconds > 0.0 || run.attempts > 1 {
                    eprintln!(
                        "download: {:.2} s waiting on the network, {} attempt(s)",
                        run.download_seconds, run.attempts
                    );
                }
                eprintln!(
                    "wrote {} rows to {} in {:.2} s",
                    run.profile.rows,
                    out.display(),
                    run.write_seconds
                );
            }
            Ok(())
        }
        Command::Checksum { dump, json, site } => {
            let sums = site.http()?.checksums(&dump)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&sums)?);
            } else {
                for (file, sum) in &sums {
                    println!("{sum}  {file}");
                }
            }
            Ok(())
        }
        Command::Latest { site } => {
            println!("{}", site.http()?.latest_dump()?);
            Ok(())
        }
        Command::Bench {
            generate: gb,
            dir,
            hit_rates,
            threads,
            kind,
            json,
        } => bench(gb, &dir, &hit_rates, threads, kind, json),
    }
}

fn print_summary(s: &Summary, targets: usize) {
    let mb = |b: u64| b as f64 / 1e6;
    eprintln!(
        "{} dump, {} targets, {} scanner threads, sha256 {}",
        s.kind, targets, s.threads, s.sha256
    );
    eprintln!(
        "records {}  hits {}  matched {}  (skipped: various {}, has master {}, status {}, entirely incorrect {})",
        s.records, s.hits, s.matched, s.skipped_various, s.skipped_master, s.skipped_status, s.skipped_quality
    );
    eprintln!(
        "in {:.1} MB  xml {:.1} MB  rows {}  decompressor {}",
        mb(s.bytes_in),
        mb(s.bytes_decompressed),
        s.rows,
        s.decompressor
    );
    let st = &s.seconds;
    eprintln!(
        "seconds: read {:.2}  hash {:.2}  decompress {:.2}  split {:.2}  scan {:.2} (cpu)  merge {:.2}  total {:.2}",
        st.read, st.hash, st.decompress, st.split, st.scan, st.merge, st.total
    );
    eprintln!("{:.0} MB/s of XML end to end", s.mb_per_s);
}

/// Writes to two writers at once.
struct Tee<A, B>(A, B);

impl<A: Write, B: Write> Write for Tee<A, B> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write_all(buf)?;
        self.1.write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()?;
        self.1.flush()
    }
}

fn decompress_only(path: &Path) -> Result<(u64, f64)> {
    let t = Instant::now();
    let mut d = flate2::bufread::MultiGzDecoder::new(BufReader::with_capacity(4 << 20, File::open(path)?));
    let mut buf = vec![0u8; 4 << 20];
    let mut n = 0u64;
    loop {
        let got = d.read(&mut buf)?;
        if got == 0 {
            break;
        }
        n += got as u64;
    }
    Ok((n, t.elapsed().as_secs_f64()))
}

fn bench(gb: f64, dir: &Path, hit_rates: &[f64], threads: usize, kind: RecordKind, json: bool) -> Result<()> {
    if gb <= 0.0 {
        bail!("--generate must be positive");
    }
    std::fs::create_dir_all(dir)?;
    let options = GenerateOptions {
        kind,
        bytes: (gb * 1e9) as u64,
        ..Default::default()
    };
    let stem = format!("synthetic-{}-{gb}g", kind.as_str());
    let xml = dir.join(format!("{stem}.xml"));
    let gz = dir.join(format!("{stem}.xml.gz"));
    if !(xml.exists() && gz.exists()) {
        eprintln!("generating {} and {} …", xml.display(), gz.display());
        let t = Instant::now();
        let plain = BufWriter::with_capacity(4 << 20, File::create(&xml)?);
        let zipped = flate2::write::GzEncoder::new(
            BufWriter::with_capacity(4 << 20, File::create(&gz)?),
            flate2::Compression::new(6),
        );
        let mut tee = Tee(plain, zipped);
        let (bytes, records) = generate(&options, &mut tee)?;
        let Tee(mut plain, zipped) = tee;
        plain.flush()?;
        zipped.finish()?.flush()?;
        eprintln!(
            "  {records} records, {:.2} GB of XML in {:.1} s",
            bytes as f64 / 1e9,
            t.elapsed().as_secs_f64()
        );
    }
    let gz_size = std::fs::metadata(&gz)?.len();
    let xml_size = std::fs::metadata(&xml)?.len();
    eprintln!(
        "xml {:.2} GB, gzip {:.2} GB (ratio {:.1})",
        xml_size as f64 / 1e9,
        gz_size as f64 / 1e9,
        xml_size as f64 / gz_size as f64
    );

    let many = ProfileOptions {
        threads,
        ..Default::default()
    }
    .scanner_threads();
    let mut results = Vec::new();

    let (n, s) = decompress_only(&gz)?;
    eprintln!(
        "decompress only (zlib-rs, 1 thread): {:.0} MB/s of XML ({:.1} s)",
        n as f64 / 1e6 / s,
        s
    );
    results.push(serde_json::json!({"run": "decompress-only", "mb_per_s": n as f64 / 1e6 / s, "seconds": s}));

    for &rate in hit_rates {
        let targets = TargetSet::from_ids(targets_for_hit_rate(rate, options.max_artist_id));
        // Split + scan on XML already decompressed (from the page cache, no
        // sha256), then the whole pipeline on the gzip file, sha256 included.
        for (label, input, no_hash) in [("split+scan (xml)", &xml, true), ("end to end (gzip)", &gz, false)] {
            for t in [1, many] {
                let options = ProfileOptions {
                    threads: t,
                    kind: Some(kind),
                    no_hash,
                    ..Default::default()
                };
                let profile = profile_path(input, &targets, &options)?;
                let s = &profile.summary;
                let hit = s.hits as f64 / s.records.max(1) as f64;
                eprintln!(
                    "{label:18} hit rate {:5.2}% ({:5.2}% measured)  {t:2} threads: {:6.0} MB/s  ({:.1} s, scan cpu {:.1} s, decompress {:.1} s, rows {})",
                    rate * 100.0,
                    hit * 100.0,
                    s.mb_per_s,
                    s.seconds.total,
                    s.seconds.scan,
                    s.seconds.decompress,
                    s.rows
                );
                results.push(serde_json::json!({
                    "run": label, "hit_rate": rate, "hit_rate_measured": hit, "threads": t,
                    "mb_per_s": s.mb_per_s, "summary": s,
                }));
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }
    Ok(())
}
