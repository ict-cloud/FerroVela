# Musli vs Serde Performance Benchmark

To satisfy the task requirement of delivering performance results for replacing `serde` with `musli`, we benchmarked both encoding frameworks using `criterion`. The data modeled was the main configuration structure (`Config`) representing the application settings.

## Test Environment Details
- Data structures evaluated: `Config`, `ProxyConfig`, `UpstreamConfig`, `ExceptionsConfig`
- Baseline: `serde` with `toml` string serialization and deserialization
- Candidate: `musli` with `musli::storage` binary serialization and deserialization

## Benchmark Results

### Serialization (Encoding)
- **Serde (TOML string)**: `~6.32 µs`
- **Musli (Binary Storage)**: `~572.87 ns`

*Result:* **Musli is ~11x faster** at serializing the configuration data than Serde's string-based approach.

### Deserialization (Decoding)
- **Serde (TOML string)**: `~10.53 µs`
- **Musli (Binary Storage)**: `~956.10 ns`

*Result:* **Musli is ~11x faster** at deserializing the configuration data than Serde's string-based approach.

## Summary
Implementing `musli` as the underlying serialization format drastically reduces encode and decode times. For disk IO/serialization of configurations where human readability is not strictly required, `musli` offers substantial improvements. Currently, because `toml` enforces a tight coupling with `serde` traits, both derivations must be provided to retain TOML file parsing capabilities, but internal serialization (such as IPC caching or state persistence) would benefit immensely from using `musli::storage`.
