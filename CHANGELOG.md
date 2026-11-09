# Changelog

## 0.1.0 — unreleased

### Added

- `discogs profile`: per-artist genre, genre/style pair and total counts
  by year, format class and promo flag from a local releases or masters
  dump (`.xml.gz` or `.xml`), with the SHA-256 checked on the way; Parquet
  or JSON lines.
- `discogs bench --generate <GB>`: a synthetic dump and timed runs per
  stage.
- `@discogs-rs/node`: `profileArtists()`, off the JS thread.
