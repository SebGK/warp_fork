# Screen-Watching Chat – Tech Spec

## Context

This feature adds a **continuous screen-watching loop** on top of the existing `computer_use` infrastructure, together with a **floating chat popup** window through which the AI model and the user exchange messages. See `PRODUCT.md` for user-facing behavior.

### Current system snapshot

| Layer | Relevant files |
|---|---|
| Screen capture engine | `crates/computer_use/src/lib.rs` – `Actor` trait, `ScreenshotParams`, `Options` |
| Recording/replay | `crates/computer_use/src/recording.rs` – `RecordingSession`, `AutomationLoop`, `StepDecision` |
| AI action types | `crates/ai/src/agent/action/mod.rs` – `AIAgentActionType` enum; `cancelled_result()`, `user_friendly_name()`, `Display` |
| AI result types | `crates/ai/src/agent/action_result/mod.rs` – `AIAgentActionResultType` enum; `is_successful()`, `is_failed()`, `is_cancelled()`, `Display` |
| Feature flags | `crates/warp_features/src/lib.rs` – `AgentModeComputerUse`, `LocalComputerUse` |
| App-level action handling | `app/src/ai/blocklist/block.rs` – handles each `AIAgentActionType` variant |
| Existing popup reference | `app/src/ai/agent/todos/popup.rs` – `AgentTodosPopupView`, inline scrollable thread, ESC binding |
| Interval utility | `crates/warp_core/src/interval_timer.rs` – `IntervalTimer` (timing only; not a tick source) |

The previous PR (`desktop-automation-agent`) established the pattern for new action types: add a request struct and a result enum, wire both exhaustive matches, and keep the AI harness integration client-side only (no protobuf conversions needed).

---

## Proposed changes

### 1. `crates/computer_use` – new `screen_watcher.rs`

Add a new module `crates/computer_use/src/screen_watcher.rs` and re-export it from `lib.rs`.

#### `ScreenWatcherConfig`

```rust
pub struct ScreenWatcherConfig {
    /// How often to capture a screenshot. Clamped to [2 s, 60 s].
    pub interval: std::time::Duration,
    /// Screenshot capture parameters (resolution caps, region).
    pub screenshot_params: ScreenshotParams,
}
```

#### `ScreenWatchEvent`

```rust
pub enum ScreenWatchEvent {
    /// A periodic screenshot was captured.
    Screenshot(Screenshot),
    /// An error occurred during capture (the watcher keeps running).
    CaptureError(String),
}
```

#### `ScreenWatcher`

```rust
pub struct ScreenWatcher {
    config: ScreenWatcherConfig,
    /// Whether the watcher is currently paused.
    paused: bool,
}

impl ScreenWatcher {
    pub fn new(config: ScreenWatcherConfig) -> Self;
    pub fn pause(&mut self);
    pub fn resume(&mut self);
    pub fn is_paused(&self) -> bool;
    pub fn set_interval(&mut self, interval: std::time::Duration);

    /// Capture a single screenshot immediately, regardless of interval.
    pub async fn capture_now(
        &self,
        actor: &mut dyn Actor,
    ) -> Result<Screenshot, String>;

    /// Run the watch loop, emitting events through `tx`.
    /// Completes when `tx` is dropped or the returned `CancellationToken` is cancelled.
    pub async fn run(
        self,
        actor: Box<dyn Actor>,
        tx: tokio::sync::mpsc::Sender<ScreenWatchEvent>,
    );
}
```

The `run` loop uses `tokio::time::interval` at the configured interval, skips a tick if the channel is full (backpressure), does not fire while `paused`, and clamps `interval` to `[2 s, 60 s]` on construction and on `set_interval`. It returns when the sender is closed.

A separate `ScreenWatchSession` wrapper (not in this crate) manages the task handle and cancellation.

#### `lib.rs` additions

```rust
pub mod screen_watcher;
pub use screen_watcher::{ScreenWatcher, ScreenWatcherConfig, ScreenWatchEvent};
```

### 2. `crates/ai` – two new action variants

Following the established pattern from `RequestDesktopRecording` / `ReplayDesktopRecording`:

#### New variants in `AIAgentActionType`

```rust
/// AI requests to start a screen-watching session.
StartScreenWatch(StartScreenWatchRequest),

/// AI requests to stop the active screen-watching session.
StopScreenWatch,
```

#### `StartScreenWatchRequest`

```rust
pub struct StartScreenWatchRequest {
    /// Suggested capture interval. Clamped to [2 s, 60 s] by the client.
    pub interval_secs: u64,
    /// Screenshot constraints.
    pub screenshot_params: Option<computer_use::ScreenshotParams>,
    /// Optional prompt the AI wants to use to open the conversation in the popup.
    pub opening_message: Option<String>,
}
```

#### New variants in `AIAgentActionResultType`

```rust
StartScreenWatch(StartScreenWatchResult),
StopScreenWatch(StopScreenWatchResult),
```

#### `StartScreenWatchResult`

```rust
pub enum StartScreenWatchResult {
    /// Session started. Field `session_id` is opaque; the client uses it to route
    /// subsequent chat messages back to the correct AI conversation.
    Started { session_id: String },
    /// Platform does not support screen capture.
    Unsupported,
    /// User denied the permission prompt.
    PermissionDenied,
    /// Another session was already active.
    AlreadyActive,
    Cancelled,
    Error(String),
}
```

#### `StopScreenWatchResult`

```rust
pub enum StopScreenWatchResult {
    /// Session was stopped.
    Stopped { messages_sent: usize },
    /// No session was active.
    NoActiveSession,
    Cancelled,
}
```

All four exhaustive match arms (`cancelled_result`, `user_friendly_name`, `Display` on `AIAgentActionType`, `Display` on `AIAgentActionResultType`) are updated following the established pattern.

### 3. `app/` – `ScreenWatchSessionModel` (new file)

Create `app/src/ai/screen_watch/mod.rs` (and a `screen_watch/` subdirectory with `model.rs`, `popup.rs`).

#### `ScreenWatchSessionModel` (model.rs)

An `Entity`-based model that owns the background watch task and the popup conversation history. Responsibilities:

- Spawns `ScreenWatcher::run(...)` as a detached Tokio task; holds an `mpsc::Sender<ScreenWatchCommand>` to issue `Pause`/`Resume`/`ChangeInterval`/`Stop` commands and an `mpsc::Receiver<ScreenWatchEvent>` for screenshot events.
- Maintains the in-memory conversation thread (`Vec<ChatMessage>`).
- Queues pending screenshots when the AI model turn is in flight; discards all but the latest queued screenshot when a new one arrives before the AI responds (PRODUCT.md invariant 42).
- Exposes a `notify_new_message(message: String, ctx)` method that the popup's input field calls.
- Emits `ScreenWatchSessionEvent` variants (`NewMessage`, `StatusChanged`, `SessionStopped`) so the popup view can re-render.

#### `ChatMessage`

```rust
pub struct ChatMessage {
    pub role: ChatRole,  // User | Agent
    pub text: String,
    pub timestamp: std::time::SystemTime,
}
```

#### `ScreenWatchStatus`

```rust
pub enum ScreenWatchStatus {
    Active,
    Paused,
    Stopped,
    DisplayUnavailable,
    Offline,
}
```

### 4. `app/` – `ScreenWatchPopupView` (popup.rs)

A new `View` type modelled on `AgentTodosPopupView`:

- Uses `warpui`'s `Overlay` / `SavePosition` to render at a configurable corner of the screen, draggable.
- Renders the `Vec<ChatMessage>` in a `ClippedScrollable` thread area.
- Renders a single-line `TextInput` at the bottom.
- Shows a status badge (colored border or icon) that reflects `ScreenWatchStatus` (PRODUCT.md invariants 28, 34).
- Pause/Resume and Stop buttons.
- An unread-message badge when minimized.
- Subscribes to the `ScreenWatchSessionModel` for `ScreenWatchSessionEvent` updates.
- On `Send` (Enter or button): calls `session_model.notify_new_message(text, ctx)`.
- Registers an ESC key binding scoped to `ScreenWatchPopupView::ui_name()` that closes/minimizes the popup (consistent with `AgentTodosPopupView`).

### 5. `app/` – action handler wiring (block.rs)

In `app/src/ai/blocklist/block.rs`, add two new match arms:

- `AIAgentActionType::StartScreenWatch(req)` → creates a `ScreenWatchSessionModel`, spawns the `ScreenWatcher`, shows the popup, returns `StartScreenWatchResult::Started { session_id }`.
- `AIAgentActionType::StopScreenWatch` → stops the active session (if any), closes the popup, returns `StopScreenWatchResult::Stopped { messages_sent }`.

The action handler must check `FeatureFlag::AgentModeComputerUse` and `FeatureFlag::LocalComputerUse`; if neither is enabled, return `StartScreenWatchResult::Unsupported`.

### 6. Permission prompt

Reuse the existing permission-prompt pattern used for `RequestComputerUse`: show a modal dialog before starting the first session. The approval is persisted in user preferences (`app/src/persistence/`). The `ScreenWatchSessionModel::start()` method checks this flag before spawning the watcher; if not approved, it shows the modal and awaits the result before proceeding.

---

## End-to-end flow

```
Agent turn
  └─ emits StartScreenWatch(req)
       └─ block.rs handler
            ├─ checks FeatureFlag
            ├─ shows permission modal (first run)
            ├─ creates ScreenWatchSessionModel
            │    └─ spawns ScreenWatcher::run() task
            ├─ creates ScreenWatchPopupView
            │    └─ anchored bottom-right, always-on-top overlay
            └─ returns StartScreenWatchResult::Started

ScreenWatcher tick (every N seconds)
  └─ ScreenWatchEvent::Screenshot(img)
       └─ ScreenWatchSessionModel
            ├─ if AI turn in flight: queue/replace pending screenshot
            └─ else: send screenshot → AI model → ChatMessage::Agent → popup re-renders

User types in popup → presses Enter
  └─ ScreenWatchSessionModel::notify_new_message()
       ├─ captures fresh screenshot via capture_now()
       ├─ sends (user message + screenshot) → AI model
       └─ ChatMessage::User appended → popup re-renders
```

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Screenshot backpressure / memory pressure on fast intervals | Queue only the latest screenshot; discard earlier pending ones before the AI responds (invariant 42). Clamp interval minimum to 2 s. |
| Simultaneous recording/replay and screen-watch | Return `AlreadyActive` / error in both directions; the same mutex-like guard used for recording/replay exclusion applies here. |
| OS screen-recording permission races on macOS | Wrap `perform_actions(&[], screenshot_params)` in a platform error handler; surface `CaptureError` events → `ScreenWatchStatus::Offline` in the popup. |
| Popup z-order on Wayland | Mark the popup window with the `always-on-top` hint; document that Wayland compositors may not honor it, consistent with how the existing overlay windows behave. |
| Warp window capture in screenshot | Document the limitation in the permission dialog; do not attempt exclusion in this iteration (PRODUCT.md invariant 36). |

---

## Testing and validation

Unit tests in `crates/computer_use/src/screen_watcher.rs` (`#[cfg(test)]` using a local `TrackingActor` that records call counts):

1. `test_screen_watcher_clamps_interval_below_min` – intervals below 2 s are clamped to 2 s (PRODUCT.md invariant 12).
2. `test_screen_watcher_clamps_interval_above_max` – intervals above 60 s are clamped to 60 s (invariant 12).
3. `test_screen_watcher_accepts_valid_interval` – a valid interval is stored unchanged.
4. `test_set_interval_clamps` – `set_interval()` clamps both too-small and too-large values.
5. `test_pause_resume_state` – `pause()`/`resume()` correctly toggle `is_paused()`.
6. `test_capture_now_while_paused` – `capture_now()` succeeds even while the watcher is paused (invariant 28 last sentence).
7. `test_screen_watcher_stops_when_channel_dropped` – `run()` returns when the event channel's receiver is dropped (invariant 30 / graceful shutdown).
8. `test_screen_watcher_stops_on_stop_command` – `run()` returns when a `Stop` command is received.
9. `test_screen_watcher_pause_skips_capture` – pausing prevents `ScreenWatchEvent::Screenshot` from being emitted (invariant 28).
10. `test_screen_watcher_resume_restarts_timer` – the interval resets to zero after `Resume`; a screenshot fires at `now + interval` after resuming (invariant 29).

Unit tests for `StartScreenWatchResult` / `StopScreenWatchResult` in `crates/ai/src/agent/action_result/mod.rs`:

6. `is_successful()` / `is_failed()` / `is_cancelled()` cover all new variants.
7. `Display` impl roundtrips for both result enums.

Manual validation mapping to PRODUCT.md invariants:

- **Invariant 3 (platform check)**: run on an unsupported platform → verify `StartScreenWatchResult::Unsupported`.
- **Invariant 4 (single session)**: attempt to start a second session → verify `AlreadyActive` error.
- **Invariant 6–8 (permission)**: first run → permission modal appears; deny → no session.
- **Invariants 11–13 (interval)**: confirm screenshots arrive at the configured interval; confirm a fresh screenshot is taken on user message.
- **Invariants 16–20 (popup)**: popup appears in correct corner; dragging persists position; ESC minimizes; clicking outside returns focus.
- **Invariants 28–29 (pause/resume)**: no screenshots while paused; interval resets on resume.
- **Invariant 34 (active indicator)**: active badge visible while session runs; disappears on stop.
- **Invariant 39 (lock screen)**: lock device → confirm no screenshots until unlock.
- **Invariant 42 (queuing)**: simulate slow AI response → confirm only the latest queued screenshot is sent.

## Definition of done

- `cargo check -p computer_use` and `cargo check -p ai` both pass.
- All new unit tests pass under `cargo test -p computer_use --features test-util` and `cargo test -p ai`.
- `AIAgentActionType` and `AIAgentActionResultType` compile without `#[allow(unreachable_patterns)]` on any existing match.
- No existing tests removed or disabled.
- `ScreenWatchPopupView` renders without panic under the noop actor path.
