# Desktop Automation Agent – Tech Spec

## Context

This tech spec covers the core engine layer for the Desktop Automation Agent feature described in `PRODUCT.md`. The feature adds two major capabilities on top of the existing `computer_use` crate:

1. **Recording** – capturing user actions and screenshots into a serializable `RecordingSession`.
2. **Replay** – executing a `RecordingSession` on the live desktop with per-step screenshot verification.

The AI agent integration (how the agent requests recording and replay) is wired through two new `AIAgentActionType` variants in the `ai` crate.

The feature is intentionally scoped to the Rust engine layer in this iteration. UI surfaces (showing a "recording active" banner, a permission prompt dialog, etc.) are tracked separately and are out of scope here.

## Affected crates

- `crates/computer_use` – new `recording.rs` module and re-exports from `lib.rs`.
- `crates/ai` – new `AIAgentActionType` variants, new result types, and updates to all exhaustive match arms.

## `computer_use` changes

### New module: `recording.rs`

#### `RecordedStep`

```rust
pub struct RecordedStep {
    pub before_screenshot: Option<Screenshot>,
    pub action: Action,
}
```

A single step in a recording. `before_screenshot` is taken immediately before the action is performed and may be `None` when screenshots are disabled (e.g. in tests). `Action` is the existing type from `lib.rs`.

#### `RecordingSession`

```rust
pub struct RecordingSession {
    steps: Vec<RecordedStep>,
    final_screenshot: Option<Screenshot>,
}
```

Collects steps during an active recording. Key methods:

- `new() -> Self` – creates an empty session.
- `add_step(&mut self, step: RecordedStep)` – appends a step.
- `finish(final_screenshot: Option<Screenshot>) -> Self` – freezes the session; called when the user signals end of recording.
- `steps(&self) -> &[RecordedStep]` – read access to steps.
- `final_screenshot(&self) -> Option<&Screenshot>` – read access to the final screenshot.
- `is_empty(&self) -> bool` – true when no steps were recorded.
- `step_count(&self) -> usize` – number of steps.

`RecordingSession` derives `Debug`, `Clone`, `PartialEq`. Screenshots are not `Eq` (they contain `Vec<u8>` that is always compared by value, which is already derived), so `RecordingSession` is `PartialEq` but not `Eq`.

Serialization (`serde::Serialize`/`Deserialize`) is derived on `RecordedStep` and `RecordingSession` for storage. `Screenshot` already derives neither `Serialize` nor `Deserialize` in the current codebase; a `SerializableScreenshot` newtype (or a dedicated serialization module using `serde_with`) is used internally if persistence is needed. For the first iteration, `RecordingSession` is only serialized when callers opt in via a feature flag; the default representation is in-memory only.

#### `AutomationLoop`

```rust
pub struct AutomationLoop<'a> {
    actor: &'a mut dyn Actor,
    session: &'a RecordingSession,
    screenshot_params: Option<ScreenshotParams>,
}
```

Drives replay. Key method:

```rust
pub async fn run<F, Fut>(
    &mut self,
    on_pre_step: F,
) -> Result<AutomationLoopResult, String>
where
    F: Fn(usize, Option<Screenshot>) -> Fut,
    Fut: Future<Output = StepDecision>,
```

For each step `i` in `session.steps()`:
1. Takes a screenshot (if `screenshot_params` is `Some`).
2. Calls `on_pre_step(i, screenshot)` and awaits the `StepDecision`.
3. If `StepDecision::Proceed`, executes the action via `actor.perform_actions(&[step.action.clone()], options)`.
4. If `StepDecision::Cancel`, stops immediately and returns `AutomationLoopResult::Cancelled { steps_completed: i }`.
5. If the action fails, returns `AutomationLoopResult::Error { message, steps_completed: i }`.
6. After all steps, takes a final screenshot if `screenshot_params` is `Some` and returns `AutomationLoopResult::Completed { steps_completed, final_screenshot }`.

#### Supporting types

```rust
pub enum StepDecision {
    Proceed,
    Cancel,
}

pub enum AutomationLoopResult {
    Completed {
        steps_completed: usize,
        final_screenshot: Option<Screenshot>,
    },
    Cancelled {
        steps_completed: usize,
    },
    Error {
        message: String,
        steps_completed: usize,
    },
}
```

All types derive `Debug` and `Clone`.

### `lib.rs` changes

Add `pub mod recording;` and re-export the public types:
```rust
pub use recording::{AutomationLoop, AutomationLoopResult, RecordedStep, RecordingSession, StepDecision};
```

## `ai` crate changes

### New action variants in `AIAgentActionType`

```rust
/// AI requests the user to record a desktop demonstration.
RequestDesktopRecording(RequestDesktopRecordingRequest),

/// AI requests replay of a previously-recorded desktop session.
ReplayDesktopRecording(ReplayDesktopRecordingRequest),
```

#### `RequestDesktopRecordingRequest`

```rust
pub struct RequestDesktopRecordingRequest {
    /// Human-readable description of the task the user should demonstrate.
    pub task_description: String,
    /// Screenshot parameters to use when capturing before-screenshots during recording.
    pub screenshot_params: Option<computer_use::ScreenshotParams>,
}
```

#### `ReplayDesktopRecordingRequest`

```rust
pub struct ReplayDesktopRecordingRequest {
    /// The recorded session to replay.
    pub recording: computer_use::RecordingSession,
    /// Screenshot parameters for pre-step verification screenshots.
    pub screenshot_params: Option<computer_use::ScreenshotParams>,
    /// If true, the client must request user confirmation before any risky action.
    pub require_confirmation_for_risky_actions: bool,
}
```

### New result variants in `AIAgentActionResultType`

```rust
RequestDesktopRecording(RequestDesktopRecordingResult),
ReplayDesktopRecording(ReplayDesktopRecordingResult),
```

#### `RequestDesktopRecordingResult`

```rust
pub enum RequestDesktopRecordingResult {
    /// Recording completed successfully.
    Success {
        recording: computer_use::RecordingSession,
    },
    /// Recording was cancelled by the user before any steps were captured.
    Cancelled,
    /// An error occurred.
    Error(String),
}
```

#### `ReplayDesktopRecordingResult`

```rust
pub enum ReplayDesktopRecordingResult {
    /// All steps completed successfully.
    Completed {
        steps_completed: usize,
        final_screenshot: Option<computer_use::Screenshot>,
    },
    /// Replay was stopped before all steps completed (user cancelled).
    StoppedEarly {
        reason: String,
        steps_completed: usize,
    },
    /// Replay was cancelled.
    Cancelled,
    /// An error occurred at the given step.
    Error {
        message: String,
        steps_completed: usize,
    },
}
```

### Updates to exhaustive matches

All four exhaustive match arms in `ai/src/agent/action/mod.rs` and `ai/src/agent/action_result/mod.rs` are updated:

- `AIAgentActionType::cancelled_result()` → returns `Cancelled` for both new variants.
- `AIAgentActionType::user_friendly_name()` → returns `"Request desktop recording"` and `"Replay desktop recording"`.
- `AIAgentActionType::Display` → uses `"RequestDesktopRecording: {task_description}"` and `"ReplayDesktopRecording: {n} steps"`.
- `AIAgentActionResultType::Display` → delegates to the result's own `Display` impl.

No protobuf (`warp_multi_agent_api`) conversions are added in this iteration; both new action types are constructed locally by the client-side agent harness and never deserialized from a server message.

## Testing

Unit tests live in `crates/computer_use/src/recording.rs` (inside `#[cfg(test)] mod tests`):

1. `test_empty_recording_session` – a new `RecordingSession` has zero steps and no final screenshot.
2. `test_add_steps` – adding two steps and calling `finish` produces a session with the correct step count and final screenshot.
3. `test_is_empty` – `is_empty()` returns `true` for a fresh session and `false` after adding a step.
4. `test_automation_loop_completes` – `AutomationLoop::run` with a two-step session and a `Proceed` decision produces `Completed { steps_completed: 2 }`.
5. `test_automation_loop_cancel` – `on_pre_step` returning `Cancel` at step 1 produces `Cancelled { steps_completed: 1 }`.
6. `test_automation_loop_empty_session` – an empty session produces `Completed { steps_completed: 0 }` immediately.

Tests use the `computer_use` crate's `test-util` feature (noop actor) to avoid requiring a real display.

## Definition of done

- `cargo check -p computer_use` and `cargo check -p ai` both pass.
- All six unit tests pass under `cargo test -p computer_use --features test-util`.
- `AIAgentActionType` and `AIAgentActionResultType` compile without `#[allow(unreachable_patterns)]` or similar suppressions on any existing match.
- No existing tests are removed or disabled.
