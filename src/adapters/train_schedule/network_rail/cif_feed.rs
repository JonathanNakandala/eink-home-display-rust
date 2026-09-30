use std::io::{BufRead, BufReader, Cursor};
use std::path::Path;

use flate2::read::GzDecoder;
use tokio::io::AsyncWriteExt;

use crate::adapters::train_schedule::network_rail::response::RecordEnvelope;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

pub async fn download_full_schedule(
    client: &reqwest::Client,
    feed_url: &str,
    username: &str,
    password: &str,
    dest: &Path,
) -> anyhow::Result<()> {
    let response = client
        .get(feed_url)
        .query(&[("type", "CIF_ALL_FULL_DAILY"), ("day", "toc-full")])
        .basic_auth(username, Some(password))
        .send()
        .await?
        .error_for_status()?;
    let bytes = response.bytes().await?;

    let mut file = tokio::fs::File::create(dest).await?;
    file.write_all(&bytes).await?;
    Ok(())
}

/// Reads `path` as newline-delimited JSON, transparently gzip-decoding it if the
/// file starts with the gzip magic bytes.
pub fn open_raw_lines(
    path: &Path,
) -> anyhow::Result<Box<dyn Iterator<Item = std::io::Result<String>>>> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(&GZIP_MAGIC) {
        let decoder = GzDecoder::new(Cursor::new(bytes));
        Ok(Box::new(BufReader::new(decoder).lines()))
    } else {
        Ok(Box::new(BufReader::new(Cursor::new(bytes)).lines()))
    }
}

/// Searches the raw feed's TIPLOC records by code or description, to help
/// configure `timing_points`.
pub fn find_tiplocs(path: &Path, term: &str) -> anyhow::Result<Vec<(String, Option<String>)>> {
    let term = term.to_uppercase();
    let mut hits = Vec::new();
    for line in open_raw_lines(path)? {
        let line = line?;
        if !line.contains("\"TiplocV1\"") {
            continue;
        }
        let Ok(envelope) = serde_json::from_str::<RecordEnvelope>(&line) else {
            continue;
        };
        let Some(tiploc) = envelope.tiploc else {
            continue;
        };
        let description = tiploc.tps_description.unwrap_or_default();
        if description.to_uppercase().contains(&term) || tiploc.tiploc_code.contains(&term) {
            hits.push((tiploc.tiploc_code, Some(description)));
        }
    }
    Ok(hits)
}
