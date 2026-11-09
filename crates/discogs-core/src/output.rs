//! Writing rows: Parquet or JSON lines.
//!
//! Both go to `<path>.partial` first and are renamed into place when
//! complete, so a failed run never leaves a half-written file under the
//! output name.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::builder::{BooleanBuilder, Float32Builder, StringBuilder, UInt16Builder, UInt32Builder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::aggregate::Row;
use crate::{Error, Result};

/// Rows per Parquet record batch.
const BATCH: usize = 1 << 20;

/// An output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Parquet,
    Jsonl,
}

impl OutputFormat {
    /// The format of a path by its extension: `.parquet`, or `.jsonl` /
    /// `.ndjson`.
    pub fn of_path(path: &Path) -> Result<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("parquet") => Ok(OutputFormat::Parquet),
            Some("jsonl" | "ndjson") => Ok(OutputFormat::Jsonl),
            _ => Err(Error::Output(format!(
                "{}: use a .parquet or .jsonl path",
                path.display()
            ))),
        }
    }
}

/// The Arrow schema of the rows.
pub fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("artist_id", DataType::UInt32, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("year", DataType::UInt16, true),
        Field::new("format_class", DataType::Utf8, true),
        Field::new("promo", DataType::Boolean, false),
        Field::new("weight", DataType::Float32, false),
        Field::new("share", DataType::Float32, false),
        Field::new("records", DataType::UInt32, false),
    ]))
}

/// The rows as Arrow record batches of up to 2^20 rows.
pub fn record_batches(rows: &[Row]) -> Result<Vec<RecordBatch>> {
    let schema = schema();
    rows.chunks(BATCH)
        .map(|chunk| {
            let mut artist = UInt32Builder::with_capacity(chunk.len());
            let mut kind = StringBuilder::new();
            let mut name = StringBuilder::new();
            let mut year = UInt16Builder::with_capacity(chunk.len());
            let mut format = StringBuilder::new();
            let mut promo = BooleanBuilder::with_capacity(chunk.len());
            let mut weight = Float32Builder::with_capacity(chunk.len());
            let mut share = Float32Builder::with_capacity(chunk.len());
            let mut records = UInt32Builder::with_capacity(chunk.len());
            for r in chunk {
                artist.append_value(r.artist_id);
                kind.append_value(r.kind.as_str());
                name.append_value(&r.name);
                year.append_option(r.year);
                format.append_option(r.format_class.map(|f| f.as_str()));
                promo.append_value(r.promo);
                weight.append_value(r.weight);
                share.append_value(r.share);
                records.append_value(r.records);
            }
            let columns: Vec<ArrayRef> = vec![
                Arc::new(artist.finish()),
                Arc::new(kind.finish()),
                Arc::new(name.finish()),
                Arc::new(year.finish()),
                Arc::new(format.finish()),
                Arc::new(promo.finish()),
                Arc::new(weight.finish()),
                Arc::new(share.finish()),
                Arc::new(records.finish()),
            ];
            RecordBatch::try_new(schema.clone(), columns).map_err(|e| Error::Output(e.to_string()))
        })
        .collect()
}

fn partial(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".partial");
    PathBuf::from(p)
}

/// Writes rows to `path` in the format its extension names.
pub fn write_rows(rows: &[Row], path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    match OutputFormat::of_path(path)? {
        OutputFormat::Parquet => write_parquet(rows, path),
        OutputFormat::Jsonl => write_jsonl(rows, path),
    }
}

/// Writes rows as Parquet (zstd).
pub fn write_parquet(rows: &[Row], path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let tmp = partial(path);
    let result = (|| {
        let file = File::create(&tmp)?;
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(
                ZstdLevel::try_new(3).map_err(|e| Error::Output(e.to_string()))?,
            ))
            .build();
        let mut writer = ArrowWriter::try_new(file, schema(), Some(props)).map_err(|e| Error::Output(e.to_string()))?;
        for batch in record_batches(rows)? {
            writer.write(&batch).map_err(|e| Error::Output(e.to_string()))?;
        }
        writer.close().map_err(|e| Error::Output(e.to_string()))?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Writes rows as JSON lines, one object per row.
pub fn write_jsonl(rows: &[Row], path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let tmp = partial(path);
    let result = (|| {
        let mut out = BufWriter::new(File::create(&tmp)?);
        for r in rows {
            serde_json::to_writer(&mut out, r).map_err(|e| Error::Output(e.to_string()))?;
            out.write_all(b"\n")?;
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
