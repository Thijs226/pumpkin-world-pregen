# Changelog

## 0.1.0

- Initial native Pumpkin WASM plugin.
- Added square pregeneration around the player's current position.
- Added bounded 8 x 8 chunk force-load batches.
- Added `/pregen status` and `/pregen cancel`.
- Added periodic and final world saves.
- Added stalled-batch timeout and cleanup.
- Hardened command and scheduler paths against re-entrant plugin callbacks.
- Added recovery for chunks that unload after a batch starts.
- Added formatting checks to CI and pinned the Pumpkin plugin API dependency.
