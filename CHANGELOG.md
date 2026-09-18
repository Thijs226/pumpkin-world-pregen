# Changelog

## 0.1.0

- Changed pregeneration to one chunk per scheduler step to avoid freezing Pumpkin with large synchronous force-load batches.
- Added console-friendly centers: `/pregen start <radius>` defaults to `0 0`, while `/pregen start <radius> <x> <z>` uses an explicit center.
- Added a guarded `/pregen trim <radius-blocks>` command. Current Pumpkin WASM APIs do not expose persisted chunk deletion yet, so the command reports the intended keep area without modifying chunks.
- Fixed the permission namespace casing required by Pumpkin (`PumpkinPregen:command.pregen`).

- Initial native Pumpkin WASM plugin.
- Added square pregeneration around the player's current position.
- Added bounded 8 x 8 chunk force-load batches.
- Added `/pregen status` and `/pregen cancel`.
- Added periodic and final world saves.
- Added stalled-batch timeout and cleanup.
- Hardened command and scheduler paths against re-entrant plugin callbacks.
- Added recovery for chunks that unload after a batch starts.
- Added formatting checks to CI and pinned the Pumpkin plugin API dependency.
