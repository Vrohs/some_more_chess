//! Loading the Lichess puzzle export into the local store.
//!
//! The export is a zstd-compressed CSV of roughly six million puzzles. It is
//! streamed rather than buffered, and filtered on the way in: the user rates
//! 1100, so anything below that is never stored.

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::grade::RATING_FLOOR;
use crate::puzzle::Puzzle;
use crate::store::Store;

/// Where the export lives. CC0 licensed.
pub const PUZZLE_DB_URL: &str = "https://database.lichess.org/lichess_db_puzzle.csv.zst";

/// Rows written per transaction.
const BATCH: usize = 10_000;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestReport {
    /// Rows read from the CSV.
    pub read: u64,
    /// Rows stored after filtering.
    pub kept: u64,
    /// Rows below the rating floor.
    pub below_floor: u64,
    /// Rows that could not be parsed.
    pub malformed: u64,
}

/// Ingest from any reader containing the decompressed CSV.
pub fn ingest_csv<R: Read>(store: &mut Store, reader: R, min_rating: u32) -> Result<IngestReport> {
    ingest_csv_reporting(store, reader, min_rating, |_| {})
}

/// As [`ingest_csv`], calling `on_batch` after each committed batch.
///
/// The full export is several million rows and takes minutes; a caller that
/// shows nothing for that long is indistinguishable from one that has hung.
pub fn ingest_csv_reporting<R: Read>(
    store: &mut Store,
    reader: R,
    min_rating: u32,
    mut on_batch: impl FnMut(&IngestReport),
) -> Result<IngestReport> {
    let mut csv = csv::ReaderBuilder::new().has_headers(true).from_reader(reader);
    let columns = Columns::from_headers(csv.headers().context("reading CSV header")?)?;

    let mut report = IngestReport::default();
    let mut batch: Vec<Puzzle> = Vec::with_capacity(BATCH);

    for record in csv.records() {
        let record = record.context("reading CSV row")?;
        report.read += 1;

        match columns.parse(&record) {
            Ok(puzzle) => {
                if puzzle.rating < min_rating {
                    report.below_floor += 1;
                    continue;
                }
                batch.push(puzzle);
                if batch.len() >= BATCH {
                    report.kept += store.insert_puzzles(&batch)? as u64;
                    batch.clear();
                    on_batch(&report);
                }
            }
            Err(_) => report.malformed += 1,
        }
    }

    if !batch.is_empty() {
        report.kept += store.insert_puzzles(&batch)? as u64;
    }
    on_batch(&report);
    Ok(report)
}

/// Ingest directly from the downloaded `.csv.zst` export.
pub fn ingest_zst_file(store: &mut Store, path: &Path, min_rating: u32) -> Result<IngestReport> {
    ingest_zst_file_reporting(store, path, min_rating, |_| {})
}

/// As [`ingest_zst_file`], reporting progress after each committed batch.
pub fn ingest_zst_file_reporting(
    store: &mut Store,
    path: &Path,
    min_rating: u32,
    on_batch: impl FnMut(&IngestReport),
) -> Result<IngestReport> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let decoder = zstd::Decoder::new(BufReader::new(file))
        .with_context(|| format!("decompressing {}", path.display()))?;
    ingest_csv_reporting(store, decoder, min_rating, on_batch)
}

/// Ingest the export using the project's rating floor.
pub fn ingest_default(
    store: &mut Store,
    path: &Path,
    on_batch: impl FnMut(&IngestReport),
) -> Result<IngestReport> {
    ingest_zst_file_reporting(store, path, RATING_FLOOR, on_batch)
}

/// Header positions, resolved once so rows are parsed by name rather than by a
/// fixed column order that the export has changed before.
struct Columns {
    index: HashMap<String, usize>,
}

impl Columns {
    fn from_headers(headers: &csv::StringRecord) -> Result<Self> {
        let index: HashMap<String, usize> = headers
            .iter()
            .enumerate()
            .map(|(i, name)| (name.trim().to_owned(), i))
            .collect();

        for required in ["PuzzleId", "FEN", "Moves", "Rating"] {
            if !index.contains_key(required) {
                bail!("puzzle CSV is missing the {required} column");
            }
        }
        Ok(Self { index })
    }

    fn get<'a>(&self, record: &'a csv::StringRecord, name: &str) -> Option<&'a str> {
        self.index.get(name).and_then(|i| record.get(*i))
    }

    fn parse(&self, record: &csv::StringRecord) -> Result<Puzzle> {
        let id = self
            .get(record, "PuzzleId")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing PuzzleId"))?
            .to_owned();
        let fen = self
            .get(record, "FEN")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("puzzle {id}: missing FEN"))?
            .to_owned();
        let moves: Vec<String> = self
            .get(record, "Moves")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if moves.len() < 2 {
            bail!("puzzle {id}: solution line is too short");
        }
        let rating = self
            .get(record, "Rating")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow!("puzzle {id}: unparseable Rating"))?;

        Ok(Puzzle {
            id,
            fen,
            moves,
            rating,
            rating_deviation: self.number(record, "RatingDeviation"),
            popularity: self
                .get(record, "Popularity")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            nb_plays: self.number(record, "NbPlays"),
            themes: self.words(record, "Themes"),
            game_url: self.get(record, "GameUrl").unwrap_or_default().to_owned(),
            opening_tags: self.words(record, "OpeningTags"),
        })
    }

    fn number(&self, record: &csv::StringRecord, name: &str) -> u32 {
        self.get(record, name).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn words(&self, record: &csv::StringRecord, name: &str) -> Vec<String> {
        self.get(record, name)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
PuzzleId,FEN,Moves,Rating,RatingDeviation,Popularity,NbPlays,Themes,GameUrl,OpeningTags,DailyDate
low00001,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,900,75,90,1000,mateIn1 backRankMate,https://lichess.org/a,,
keep0001,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1100,80,95,2000,mateIn1,https://lichess.org/b,,
keep0002,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8 b1b8,1500,70,85,3000,fork pin,https://lichess.org/c,Sicilian_Defense,
bad00001,,,,,,,,,,
short001,6k1/5ppp/8/8/8/8/5PPP/1R4K1 b - - 0 1,g8h8,1400,70,85,3000,mateIn1,https://lichess.org/d,,
";

    fn ingest_sample() -> (Store, IngestReport) {
        let mut store = Store::in_memory().unwrap();
        let report = ingest_csv(&mut store, SAMPLE.as_bytes(), RATING_FLOOR).unwrap();
        (store, report)
    }

    #[test]
    fn puzzles_below_the_rating_floor_are_dropped() {
        let (store, report) = ingest_sample();
        assert_eq!(report.below_floor, 1);
        assert!(store.puzzle("low00001").unwrap().is_none());
        assert!(store.puzzle("keep0001").unwrap().is_some());
    }

    #[test]
    fn a_puzzle_at_exactly_the_floor_is_kept() {
        let (store, _) = ingest_sample();
        let puzzle = store.puzzle("keep0001").unwrap().unwrap();
        assert_eq!(puzzle.rating, RATING_FLOOR);
    }

    #[test]
    fn malformed_rows_are_counted_not_fatal() {
        let (store, report) = ingest_sample();
        // The empty row and the one-move row are both unusable.
        assert_eq!(report.malformed, 2);
        assert_eq!(report.read, 5);
        assert_eq!(store.count_puzzles().unwrap(), 2);
    }

    #[test]
    fn themes_and_openings_are_split_into_lists() {
        let (store, _) = ingest_sample();
        let puzzle = store.puzzle("keep0002").unwrap().unwrap();
        assert_eq!(puzzle.themes, vec!["fork", "pin"]);
        assert_eq!(puzzle.opening_tags, vec!["Sicilian_Defense"]);
        assert_eq!(puzzle.moves, vec!["g8h8", "b1b8"]);
    }

    #[test]
    fn ingesting_twice_is_idempotent() {
        let (mut store, _) = ingest_sample();
        ingest_csv(&mut store, SAMPLE.as_bytes(), RATING_FLOOR).unwrap();
        assert_eq!(store.count_puzzles().unwrap(), 2);
    }

    #[test]
    fn progress_is_reported_for_the_final_partial_batch() {
        let mut store = Store::in_memory().unwrap();
        let mut seen = Vec::new();
        ingest_csv_reporting(&mut store, SAMPLE.as_bytes(), RATING_FLOOR, |report| {
            seen.push(report.kept)
        })
        .unwrap();
        // Fewer rows than one batch, so the only callback is the closing one.
        assert_eq!(seen, vec![2]);
    }

    #[test]
    fn a_missing_required_column_is_rejected() {
        let mut store = Store::in_memory().unwrap();
        let err = ingest_csv(&mut store, "PuzzleId,FEN\nx,y\n".as_bytes(), RATING_FLOOR)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Moves"), "unexpected error: {err}");
    }
}
