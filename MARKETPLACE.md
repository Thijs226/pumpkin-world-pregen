# Marketplace description

## Short description

Native world pregenerator for Pumpkin. Useful for terrain-generation analysis, testing, and pre-generating a defined area.

## Full description

PumpkinPregen pregenerates world chunks on Pumpkin using bounded batches so large areas can be generated without requesting everything at once.

It is not required for normal server performance. I originally made it because I needed a repeatable way to pregenerate larger areas while analysing Pumpkin's terrain generation, then decided to publish it in case other server owners or Pumpkin developers find it useful.

Commands:

- `/pregen start <radius-blocks>`
- `/pregen status`
- `/pregen cancel`

The plugin handles batch cleanup, progress tracking, periodic saving, cancellation, and stalled-batch timeouts.

The initial implementation was AI-assisted with GPT-5.6 Sol, then reviewed, API-checked, hardened, and tested before publication. It is not an untested AI code dump.

Source code is public on GitHub under the MIT license.
