use std::sync::{Arc, Mutex};

use pumpkin_plugin_api::{
    Context, Plugin, PluginMetadata, Server, register_plugin,
    command::{
        Arg, ArgumentType, Command, CommandError, CommandNode, CommandSender, ConsumedArgs,
    },
    command_wit::Number,
    commands::CommandHandler,
    permission::{Permission, PermissionDefault, PermissionLevel},
    scheduler::{SchedulerExt, cancel_task},
    server::CommandSender as ServerCommandSender,
    text::TextComponent,
};
use tracing::{info, warn};

const PERMISSION: &str = "pumpkinpregen:command.pregen";
const BATCH_SIDE: i32 = 8;
const BATCH_POLL_TIMEOUT_TICKS: u32 = 20 * 120;
const SAVE_EVERY_CHUNKS: u64 = 1024;
const MIN_RADIUS: i32 = 0;
const MAX_RADIUS: i32 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Batch {
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
}

impl Batch {
    fn chunk_count(self) -> u64 {
        let width = i64::from(self.max_x) - i64::from(self.min_x) + 1;
        let depth = i64::from(self.max_z) - i64::from(self.min_z) + 1;
        (width * depth) as u64
    }

    fn forceload_coordinates(self) -> (i32, i32, i32, i32) {
        (
            self.min_x.saturating_mul(16),
            self.min_z.saturating_mul(16),
            self.max_x.saturating_mul(16),
            self.max_z.saturating_mul(16),
        )
    }
}

#[derive(Debug)]
struct Job {
    world_name: String,
    dimension: String,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    next_x: i32,
    next_z: i32,
    active_batch: Option<Batch>,
    active_forceloads: Vec<Batch>,
    active_wait_ticks: u32,
    completed_chunks: u64,
    total_chunks: u64,
    last_saved_chunks: u64,
    cancel_requested: bool,
    task_id: Option<u32>,
}

impl Job {
    fn next_batch(&mut self) -> Option<Batch> {
        if self.next_z > self.max_z {
            return None;
        }

        let batch = Batch {
            min_x: self.next_x,
            min_z: self.next_z,
            max_x: (self.next_x + BATCH_SIDE - 1).min(self.max_x),
            max_z: (self.next_z + BATCH_SIDE - 1).min(self.max_z),
        };

        self.next_x += BATCH_SIDE;
        if self.next_x > self.max_x {
            self.next_x = self.min_x;
            self.next_z += BATCH_SIDE;
        }

        self.active_batch = Some(batch);
        self.active_wait_ticks = 0;
        Some(batch)
    }

    fn percent(&self) -> f64 {
        if self.total_chunks == 0 {
            100.0
        } else {
            (self.completed_chunks as f64 / self.total_chunks as f64) * 100.0
        }
    }
}

#[derive(Default)]
struct State {
    job: Option<Job>,
    last_result: Option<String>,
}

type SharedState = Arc<Mutex<State>>;

pub struct PumpkinPregen {
    state: SharedState,
}

impl Plugin for PumpkinPregen {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "PumpkinPregen".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            authors: vec!["Thijs226".into()],
            description: "Native chunk pregeneration for Pumpkin servers".into(),
            dependencies: vec![],
            permissions: vec![],
        }
    }

    fn on_load(&self, context: Context) -> pumpkin_plugin_api::Result<()> {
        context.register_permission(&Permission {
            node: PERMISSION.into(),
            description: "Allows controlling PumpkinPregen".into(),
            default: PermissionDefault::Op(PermissionLevel::Three),
            children: vec![],
        })?;

        let command = Command::new(
            &["pregen".to_string(), "pumpkinpregen".to_string()],
            "Pregenerate chunks around your current position",
        )
        .then(CommandNode::literal("status").execute(StatusCommand {
            state: Arc::clone(&self.state),
        }))
        .then(CommandNode::literal("cancel").execute(CancelCommand {
            state: Arc::clone(&self.state),
        }))
        .then(
            CommandNode::literal("start").then(
                CommandNode::argument(
                    "radius",
                    &ArgumentType::Integer((Some(MIN_RADIUS), Some(MAX_RADIUS))),
                )
                .execute(StartCommand {
                    state: Arc::clone(&self.state),
                }),
            ),
        )
        .execute(HelpCommand);

        context.register_command(command, PERMISSION);
        info!("PumpkinPregen {} loaded", env!("CARGO_PKG_VERSION"));
        Ok(())
    }

    fn on_unload(&self, context: Context) -> pumpkin_plugin_api::Result<()> {
        let cleanup = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.job.take().map(|job| {
                (
                    job.dimension,
                    job.world_name,
                    job.active_forceloads,
                    job.task_id,
                )
            })
        };

        if let Some((dimension, world_name, active_forceloads, task_id)) = cleanup {
            let server = context.get_server();
            remove_forceloads(&server, &dimension, &active_forceloads);
            if let Some(world) = server.get_world_by_name(&world_name) {
                let _ = world.save();
            }
            if let Some(task_id) = task_id {
                cancel_task(task_id);
            }
        }

        Ok(())
    }
}

struct HelpCommand;

impl CommandHandler for HelpCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        send(&sender, "PumpkinPregen commands:");
        send(&sender, "/pregen start <radius-blocks>");
        send(&sender, "/pregen status");
        send(&sender, "/pregen cancel");
        Ok(1)
    }
}

struct StartCommand {
    state: SharedState,
}

impl CommandHandler for StartCommand {
    fn handle(
        &self,
        sender: CommandSender,
        server: Server,
        args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let Some(radius) = read_i32(&args, "radius") else {
            send_error(&sender, "Invalid radius.");
            return Ok(0);
        };

        let Some((x, _, z)) = sender.position() else {
            send_error(&sender, "Run /pregen start in-game so a center position is available.");
            return Ok(0);
        };
        let Some(world) = sender.world() else {
            send_error(&sender, "Could not determine your current world.");
            return Ok(0);
        };

        let world_name = world.get_name();
        let dimension = world.get_dimension();

        // Pumpkin's current public plugin API cannot directly request an unloaded chunk.
        // This plugin uses the server's native /forceload command through `execute in`.
        // `execute in` resolves the first loaded world with a matching dimension, so reject
        // duplicate/custom worlds that share the same dimension to avoid touching the wrong world.
        let dimension_world = server
            .get_all_worlds()
            .into_iter()
            .find(|candidate| candidate.get_dimension() == dimension)
            .map(|candidate| candidate.get_name());

        if dimension_world.as_deref() != Some(world_name.as_str()) {
            send_error(
                &sender,
                "This Pumpkin version cannot safely pregenerate a custom world that shares a dimension. Use the primary Overworld, Nether, or End for now.",
            );
            return Ok(0);
        }

        let running_message = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.job.as_ref().map(|job| {
                format!(
                    "A pregeneration is already running in {}: {:.1}% ({}/{})",
                    job.world_name,
                    job.percent(),
                    job.completed_chunks,
                    job.total_chunks
                )
            })
        };
        if let Some(message) = running_message {
            send(&sender, &message);
            return Ok(0);
        }

        let center_x = floor_to_i32(x);
        let center_z = floor_to_i32(z);
        let min_block_x = center_x.saturating_sub(radius);
        let max_block_x = center_x.saturating_add(radius);
        let min_block_z = center_z.saturating_sub(radius);
        let max_block_z = center_z.saturating_add(radius);

        let min_x = min_block_x.div_euclid(16);
        let max_x = max_block_x.div_euclid(16);
        let min_z = min_block_z.div_euclid(16);
        let max_z = max_block_z.div_euclid(16);

        let width = i64::from(max_x) - i64::from(min_x) + 1;
        let depth = i64::from(max_z) - i64::from(min_z) + 1;
        let total_chunks = (width * depth) as u64;

        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.last_result = None;
            state.job = Some(Job {
                world_name: world_name.clone(),
                dimension: dimension.clone(),
                min_x,
                min_z,
                max_x,
                max_z,
                next_x: min_x,
                next_z: min_z,
                active_batch: None,
                active_forceloads: Vec::new(),
                active_wait_ticks: 0,
                completed_chunks: 0,
                total_chunks,
                last_saved_chunks: 0,
                cancel_requested: false,
                task_id: None,
            });
        }

        let shared = Arc::clone(&self.state);
        let task_id = server.schedule_repeating_task(1, 1, move |server| {
            tick_job(&server, &shared);
        });

        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = &mut state.job {
                job.task_id = Some(task_id);
            }
        }

        send(
            &sender,
            &format!(
                "Started pregenerating {} chunks in {} using {}x{} chunk batches.",
                total_chunks, world_name, BATCH_SIDE, BATCH_SIDE
            ),
        );
        send(
            &sender,
            "Use /pregen status for progress or /pregen cancel to stop safely.",
        );

        Ok(1)
    }
}

struct StatusCommand {
    state: SharedState,
}

impl CommandHandler for StatusCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let message = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = &state.job {
                let active = job
                    .active_batch
                    .map(|b| {
                        format!(
                            " active batch [{}, {}] to [{}, {}]",
                            b.min_x, b.min_z, b.max_x, b.max_z
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "{}: {:.2}% complete, {}/{} chunks.{}",
                    job.world_name,
                    job.percent(),
                    job.completed_chunks,
                    job.total_chunks,
                    active
                )
            } else if let Some(result) = &state.last_result {
                result.clone()
            } else {
                "No pregeneration job is running.".to_string()
            }
        };
        send(&sender, &message);
        Ok(1)
    }
}

struct CancelCommand {
    state: SharedState,
}

impl CommandHandler for CancelCommand {
    fn handle(
        &self,
        sender: CommandSender,
        _server: Server,
        _args: ConsumedArgs,
    ) -> Result<i32, CommandError> {
        let cancellation_requested = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(job) = &mut state.job {
                job.cancel_requested = true;
                true
            } else {
                false
            }
        };

        if cancellation_requested {
            send(
                &sender,
                "Cancellation requested. The current batch will be cleaned up safely.",
            );
            Ok(1)
        } else {
            send(&sender, "No pregeneration job is running.");
            Ok(0)
        }
    }
}

#[derive(Debug)]
enum TickAction {
    StartBatch {
        world_name: String,
        dimension: String,
        batch: Batch,
    },
    PollBatch {
        world_name: String,
        dimension: String,
        batch: Batch,
    },
    Finish {
        world_name: String,
        task_id: Option<u32>,
        completed_chunks: u64,
    },
    Cancel {
        world_name: String,
        dimension: String,
        forceloads: Vec<Batch>,
        task_id: Option<u32>,
        completed_chunks: u64,
        total_chunks: u64,
    },
}

fn tick_job(server: &Server, shared: &SharedState) {
    let action = {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = state.job.as_ref() else {
            return;
        };

        if job.cancel_requested {
            let Some(job) = state.job.take() else {
                return;
            };
            TickAction::Cancel {
                world_name: job.world_name,
                dimension: job.dimension,
                forceloads: job.active_forceloads,
                task_id: job.task_id,
                completed_chunks: job.completed_chunks,
                total_chunks: job.total_chunks,
            }
        } else {
            let next_action = {
                let Some(job) = state.job.as_mut() else {
                    return;
                };
                if let Some(batch) = job.active_batch {
                    job.active_wait_ticks = job.active_wait_ticks.saturating_add(1);
                    Some(TickAction::PollBatch {
                        world_name: job.world_name.clone(),
                        dimension: job.dimension.clone(),
                        batch,
                    })
                } else if let Some(batch) = job.next_batch() {
                    Some(TickAction::StartBatch {
                        world_name: job.world_name.clone(),
                        dimension: job.dimension.clone(),
                        batch,
                    })
                } else {
                    None
                }
            };

            if let Some(action) = next_action {
                action
            } else {
                let Some(job) = state.job.take() else {
                    return;
                };
                TickAction::Finish {
                    world_name: job.world_name,
                    task_id: job.task_id,
                    completed_chunks: job.completed_chunks,
                }
            }
        }
    };

    match action {
        TickAction::StartBatch {
            world_name,
            dimension,
            batch,
        } => {
            let Some(world) = server.get_world_by_name(&world_name) else {
                fail_job(
                    server,
                    shared,
                    &format!("World {world_name} disappeared while pregenerating."),
                );
                return;
            };

            // Do not touch chunks that are already loaded. In particular, this preserves
            // pre-existing /forceload chunks because they should already be resident. Reserve
            // the runs in state before issuing host calls so a re-entrant scheduler callback
            // cannot claim the same chunks twice.
            let candidates = unloaded_runs(&world, batch);
            let forceloads = claim_forceload_runs(shared, batch, &candidates);
            for run in &forceloads {
                add_forceload(server, &dimension, *run);
            }
        }
        TickAction::PollBatch {
            world_name,
            dimension,
            batch,
        } => {
            let Some(world) = server.get_world_by_name(&world_name) else {
                fail_job(
                    server,
                    shared,
                    &format!("World {world_name} disappeared while pregenerating."),
                );
                return;
            };

            let timed_out = {
                let state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state
                    .job
                    .as_ref()
                    .is_some_and(|job| job.active_wait_ticks >= BATCH_POLL_TIMEOUT_TICKS)
            };

            if timed_out {
                fail_job(
                    server,
                    shared,
                    "A chunk batch did not finish loading within 120 seconds.",
                );
                return;
            }

            // A chunk can be loaded by a nearby player when the batch starts, then unload
            // after the player moves away. Pick up those newly-unloaded chunks without
            // touching chunks that stayed loaded (including pre-existing force-loads).
            let requested = {
                let state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state
                    .job
                    .as_ref()
                    .filter(|job| job.active_batch == Some(batch))
                    .map(|job| job.active_forceloads.clone())
                    .unwrap_or_default()
            };
            let candidates = unrequested_unloaded_runs(&world, batch, &requested);
            let newly_claimed = claim_forceload_runs(shared, batch, &candidates);
            for run in &newly_claimed {
                add_forceload(server, &dimension, *run);
            }

            if !batch_is_loaded(&world, batch) {
                return;
            }

            // Claim completion while holding the state lock. This makes repeating task
            // re-entry harmless: only one callback can clear this active batch.
            let Some((forceloads, should_save, completed, total)) = ({
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                let Some(job) = &mut state.job else {
                    return;
                };
                if job.active_batch != Some(batch) {
                    None
                } else {
                    let forceloads = std::mem::take(&mut job.active_forceloads);
                    job.completed_chunks = job.completed_chunks.saturating_add(batch.chunk_count());
                    job.active_batch = None;
                    job.active_wait_ticks = 0;

                    let should_save = job.completed_chunks.saturating_sub(job.last_saved_chunks)
                        >= SAVE_EVERY_CHUNKS;
                    if should_save {
                        job.last_saved_chunks = job.completed_chunks;
                    }
                    Some((forceloads, should_save, job.completed_chunks, job.total_chunks))
                }
            }) else {
                return;
            };

            remove_forceloads(server, &dimension, &forceloads);

            if should_save
                && let Err(error) = world.save()
            {
                warn!("Periodic world save failed during pregeneration: {error}");
            }

            if completed == total || completed % 4096 < batch.chunk_count() {
                info!("PumpkinPregen progress: {completed}/{total} chunks");
            }
        }
        TickAction::Finish {
            world_name,
            task_id,
            completed_chunks,
        } => {
            if let Some(world) = server.get_world_by_name(&world_name)
                && let Err(error) = world.save()
            {
                warn!("Final world save failed after pregeneration: {error}");
            }
            if let Some(task_id) = task_id {
                cancel_task(task_id);
            }

            let result = format!(
                "Pregeneration complete in {world_name}: {completed_chunks} chunks generated/loaded."
            );
            {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.last_result = Some(result.clone());
            }
            info!("{result}");
        }
        TickAction::Cancel {
            world_name,
            dimension,
            forceloads,
            task_id,
            completed_chunks,
            total_chunks,
        } => {
            remove_forceloads(server, &dimension, &forceloads);
            if let Some(world) = server.get_world_by_name(&world_name) {
                let _ = world.save();
            }
            if let Some(task_id) = task_id {
                cancel_task(task_id);
            }

            let result = format!(
                "Pregeneration cancelled in {world_name} at {completed_chunks}/{total_chunks} chunks."
            );
            {
                let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
                state.last_result = Some(result.clone());
            }
            info!("{result}");
        }
    }
}

fn fail_job(server: &Server, shared: &SharedState, reason: &str) {
    let cleanup = {
        let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(job) = state.job.take() else {
            return;
        };
        (
            job.world_name,
            job.dimension,
            job.active_forceloads,
            job.task_id,
        )
    };

    let (world_name, dimension, forceloads, task_id) = cleanup;
    remove_forceloads(server, &dimension, &forceloads);
    if let Some(world) = server.get_world_by_name(&world_name) {
        let _ = world.save();
    }
    if let Some(task_id) = task_id {
        cancel_task(task_id);
    }

    let result = format!("PumpkinPregen stopped: {reason}");
    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
    state.last_result = Some(result.clone());
    warn!("{result}");
}

fn batch_is_loaded(world: &pumpkin_plugin_api::world::World, batch: Batch) -> bool {
    for x in batch.min_x..=batch.max_x {
        for z in batch.min_z..=batch.max_z {
            if world.get_chunk(x, z).is_none() {
                return false;
            }
        }
    }
    true
}

fn unloaded_runs(world: &pumpkin_plugin_api::world::World, batch: Batch) -> Vec<Batch> {
    let mut runs = Vec::new();

    for z in batch.min_z..=batch.max_z {
        let mut run_start = None;
        for x in batch.min_x..=batch.max_x {
            let loaded = world.get_chunk(x, z).is_some();
            match (run_start, loaded) {
                (None, false) => run_start = Some(x),
                (Some(start), true) => {
                    runs.push(Batch {
                        min_x: start,
                        min_z: z,
                        max_x: x - 1,
                        max_z: z,
                    });
                    run_start = None;
                }
                _ => {}
            }
        }

        if let Some(start) = run_start {
            runs.push(Batch {
                min_x: start,
                min_z: z,
                max_x: batch.max_x,
                max_z: z,
            });
        }
    }

    runs
}

fn batch_contains(batch: Batch, x: i32, z: i32) -> bool {
    x >= batch.min_x && x <= batch.max_x && z >= batch.min_z && z <= batch.max_z
}

fn covered_by_any(runs: &[Batch], x: i32, z: i32) -> bool {
    runs.iter().copied().any(|run| batch_contains(run, x, z))
}

fn claim_forceload_runs(shared: &SharedState, batch: Batch, candidates: &[Batch]) -> Vec<Batch> {
    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
    let Some(job) = &mut state.job else {
        return Vec::new();
    };
    if job.active_batch != Some(batch) {
        return Vec::new();
    }

    let claimed = uncovered_runs(candidates, &job.active_forceloads);
    job.active_forceloads.extend(claimed.iter().copied());
    claimed
}

fn uncovered_runs(candidates: &[Batch], requested: &[Batch]) -> Vec<Batch> {
    let mut runs = Vec::new();

    for candidate in candidates {
        for z in candidate.min_z..=candidate.max_z {
            let mut run_start = None;
            for x in candidate.min_x..=candidate.max_x {
                let covered = covered_by_any(requested, x, z);
                match (run_start, covered) {
                    (None, false) => run_start = Some(x),
                    (Some(start), true) => {
                        runs.push(Batch {
                            min_x: start,
                            min_z: z,
                            max_x: x - 1,
                            max_z: z,
                        });
                        run_start = None;
                    }
                    _ => {}
                }
            }

            if let Some(start) = run_start {
                runs.push(Batch {
                    min_x: start,
                    min_z: z,
                    max_x: candidate.max_x,
                    max_z: z,
                });
            }
        }
    }

    runs
}

fn unrequested_unloaded_runs(
    world: &pumpkin_plugin_api::world::World,
    batch: Batch,
    requested: &[Batch],
) -> Vec<Batch> {
    let mut runs = Vec::new();

    for z in batch.min_z..=batch.max_z {
        let mut run_start = None;
        for x in batch.min_x..=batch.max_x {
            let needs_forceload =
                world.get_chunk(x, z).is_none() && !covered_by_any(requested, x, z);
            match (run_start, needs_forceload) {
                (None, true) => run_start = Some(x),
                (Some(start), false) => {
                    runs.push(Batch {
                        min_x: start,
                        min_z: z,
                        max_x: x - 1,
                        max_z: z,
                    });
                    run_start = None;
                }
                _ => {}
            }
        }

        if let Some(start) = run_start {
            runs.push(Batch {
                min_x: start,
                min_z: z,
                max_x: batch.max_x,
                max_z: z,
            });
        }
    }

    runs
}

fn remove_forceloads(server: &Server, dimension: &str, runs: &[Batch]) {
    for run in runs {
        remove_forceload(server, dimension, *run);
    }
}

fn add_forceload(server: &Server, dimension: &str, batch: Batch) {
    let (x1, z1, x2, z2) = batch.forceload_coordinates();
    let command =
        format!("execute in {dimension} run forceload add {x1} {z1} {x2} {z2}");
    server.execute_command(&command, ServerCommandSender::Console);
}

fn remove_forceload(server: &Server, dimension: &str, batch: Batch) {
    let (x1, z1, x2, z2) = batch.forceload_coordinates();
    let command =
        format!("execute in {dimension} run forceload remove {x1} {z1} {x2} {z2}");
    server.execute_command(&command, ServerCommandSender::Console);
}

fn read_i32(args: &ConsumedArgs, key: &str) -> Option<i32> {
    match args.get_value(key) {
        Arg::Num(Ok(Number::Int32(value))) => Some(value),
        Arg::Num(Ok(Number::Int64(value))) => i32::try_from(value).ok(),
        _ => None,
    }
}

fn floor_to_i32(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn send(sender: &CommandSender, message: &str) {
    sender.send_message(TextComponent::text(message));
}

fn send_error(sender: &CommandSender, message: &str) {
    sender.send_error(TextComponent::text(message));
}

register_plugin!(PumpkinPregen);
