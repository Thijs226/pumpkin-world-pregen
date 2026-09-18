# PumpkinPregen

Native world pregeneration for [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin), written as a Rust WebAssembly plugin.

PumpkinPregen is **not required for normal server performance**. I originally made it because I needed a repeatable way to pregenerate larger areas while analysing Pumpkin's terrain generation, and decided to publish it in case it is useful to other server owners or Pumpkin developers.

The initial implementation was AI-assisted with **GPT-5.6 Sol**, then reviewed, hardened against Pumpkin's current plugin API, and tested before publication. It is not an untested code dump.

## Why this exists

Pumpkin does not currently expose a public plugin call that directly requests generation of an unloaded chunk. `World::get_chunk(x, z)` only reports chunks that are already loaded.

PumpkinPregen works with the current API by using Pumpkin's own `/forceload` command in bounded batches. It then waits until every chunk in the batch appears through `World::get_chunk`, removes the force-load tickets, and moves to the next batch.

This means the actual loading and generation still goes through Pumpkin's normal chunk system.

## Commands

```text
/pregen start <radius-blocks>
/pregen trim <radius-blocks>
/pregen status
/pregen cancel
```

`radius-blocks` is measured outward from the player's current X/Z position. The generated area is square, similar to a square Chunky selection. A radius of `0` is useful as a one-chunk smoke test.

The command permission is:

```text
PumpkinPregen:command.pregen
```

It defaults to permission level 3 operators.

## Safety and load control

- Generates at most an 8 x 8 chunk force-load batch at a time.
- Waits for a batch to finish before requesting the next one.
- Skips chunks that are already loaded, so it does not intentionally disturb existing force-loaded chunks.
- If a player-loaded chunk unloads while its batch is still running, PumpkinPregen notices it and temporarily force-loads that chunk so the job cannot stall.
- Tracks and removes only the force-load runs PumpkinPregen itself requested.
- Saves every 1,024 completed chunks and once again at completion. Pumpkin also persists generated or dirty chunks when they unload.
- `/pregen cancel` cleans up the active batch before stopping.
- Normal plugin unload also removes the active force-load batch.
- A batch that is stuck for 120 seconds is cleaned up and the job stops instead of hanging forever.

### Trim command

`/pregen trim <radius-blocks>` is registered now, but on current Pumpkin builds it is intentionally guarded and performs no deletion. Pumpkin's WASM plugin API does not yet expose safe deletion of persisted chunks or region entries.

The command reports the square area that would be kept and explicitly states that no chunks were modified. Once Pumpkin exposes a safe chunk-deletion API, this command can become a real Chunky-style trim without changing its syntax.

## Current limitation

Pumpkin's `/execute in <dimension>` chooses the first loaded world using that dimension. Because of that, version 0.1.0 deliberately rejects a custom world when another world with the same dimension is loaded. The primary Overworld, Nether, and End are supported.

Once Pumpkin exposes a public `load_chunk` or `request_chunk` method to WASM plugins, this workaround can be replaced with a direct API call and custom worlds can be supported safely.

A hard process crash can still happen between adding and removing a force-load run. If that happens, the last run may remain force-loaded after restart. This is an upstream API limitation in this first release and should be documented for server owners.

PumpkinPregen avoids modifying chunks that are already loaded when a batch starts. This protects normal pre-existing `/forceload` chunks in ordinary operation because forced chunks are expected to stay loaded. There is still a narrow startup race if Pumpkin has recorded a forced chunk but has not loaded it yet when PumpkinPregen begins.

## Building

Install Rust and the WASI Preview 2 target:

```bash
rustup target add wasm32-wasip2
cargo build --release
```

The plugin will be at:

```text
target/wasm32-wasip2/release/pumpkin_pregen.wasm
```

Place that `.wasm` file in Pumpkin's plugin directory.

## Automated build

The included GitHub Actions workflow checks formatting and builds the WASM file on every push and pull request. Tags starting with `v` also produce a downloadable GitHub Release artifact.

## Compatibility

This first version targets:

```text
pumpkin-plugin-api = "=0.1.0-dev+26.2-26.45"
```

Pumpkin is still changing quickly, so API version bumps may require small source updates.

## License

MIT

## Publishing

The project is ready for a normal public GitHub repository. Push the source, then let the included **Build** workflow compile the WASM artifact. Create a tag such as `v0.1.0` to run the **Release** workflow and attach `PumpkinPregen.wasm` to a GitHub Release.

Pumpkin can load unsigned plugins when the server permits them. Official Pumpkin Marketplace distribution uses Pumpkin's WASM signing and marketplace metadata pipeline, so marketplace signing should be done as the publication step rather than by committing a private signing key to this repository.

Before calling the first release stable, test at least a radius of `0`, `64`, and a few hundred blocks on a disposable world and verify that `/forceload query` is empty after completion or cancellation.
