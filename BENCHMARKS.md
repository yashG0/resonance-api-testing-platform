# Performance measurements

These are baseline observations, not a comparison with Postman or Bruno. They are machine- and build-dependent and should not be used as general product claims.

## Available startup sample

The checked-in `cosmic-api-benchmark.json` records five launches of `./target/release/cosmic-api`:

| Measure | Observed |
|---|---:|
| Mean process time | 719 ms |
| Median process time | 551 ms |
| Minimum / maximum | 487 ms / 1,174 ms |
| Recorded memory usage | 115,638,272 bytes per sample (about 110.3 MiB) |

The capture does not identify the machine, OS, display backend, release commit, or measurement tool. Process time is not a verified cold-start-to-first-frame measurement, and the memory sample is not a controlled idle-RSS measurement. Treat these values as a rough historical baseline only.

## Measurements still needed

For a reproducible baseline, record the app version and commit, OS and hardware, renderer, build profile, measurement tool, and number of repetitions. Measure separately:

- cold launch to first usable frame;
- idle resident memory after a fixed settling period;
- resident memory and interaction latency after loading a 5 MiB response;
- rendering time and memory for a 2,000-row history dataset.

Run each measurement on representative Linux, Windows, and macOS hardware. Compare with other clients only under documented, equivalent conditions. Do not publish a cross-product performance claim until those measurements exist.
