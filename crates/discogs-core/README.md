# discogs-core

The engine of [discogs-rs](https://github.com/the-haus/discogs-rs): reads a
Discogs monthly dump (`discogs_YYYYMMDD_releases.xml.gz`, `…_masters.xml.gz`)
and aggregates per-artist genre and style profiles, on every core, in flat
memory. `records` hands over the raw XML of the `masters`, `artists` and
`labels` records whose ids are in a set, in file order. See the repository
README for the output schema and the records rules.

Feature `http` (on by default) reads dumps straight from data.discogs.com
(`reqwest` blocking over `rustls`/ring), with `CHECKSUM.txt` lookups, the
latest dump, whole-file retries and skip-unchanged (`run::run_profile`).
Build with `default-features = false` for the local-file engine alone.
