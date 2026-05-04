//! Continuous screen-watching loop.
//!
//! This module provides:
//! - [`ScreenWatcherConfig`]: configuration for interval and screenshot constraints.
//! - [`ScreenWatchEvent`]: events emitted by the watcher (captured screenshot or error).
//! - [`ScreenWatchCommand`]: commands sent to a running watcher to pause, resume, or stop it.
//! - [`ScreenWatcher`]: drives periodic screenshot capture and forwards results on a channel.

use std::time::Duration;

use crate::{Actor, ActionResult, Options, Screenshot, ScreenshotParams};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MIN_INTERVAL: Duration = Duration::from_secs(2);
const MAX_INTERVAL: Duration = Duration::from_secs(60);

fn clamp_interval(d: Duration) -> Duration {
    d.clamp(MIN_INTERVAL, MAX_INTERVAL)
}

// ---------------------------------------------------------------------------
// ScreenWatcherConfig
// ---------------------------------------------------------------------------

/// Configuration for a [`ScreenWatcher`] session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenWatcherConfig {
    /// How often to capture a screenshot.
    ///
    /// Values are clamped to [`MIN_INTERVAL`]–[`MAX_INTERVAL`] (2 s – 60 s).
    pub interval: Duration,
    /// Constraints applied to each captured screenshot (resolution caps, region).
    pub screenshot_params: ScreenshotParams,
}

// ---------------------------------------------------------------------------
// ScreenWatchEvent
// ---------------------------------------------------------------------------

/// Events emitted by a running [`ScreenWatcher`].
#[derive(Debug, Clone)]
pub enum ScreenWatchEvent {
    /// A periodic (or on-demand) screenshot was captured.
    Screenshot(Screenshot),
    /// Screenshot capture failed for this interval. The watcher continues running.
    CaptureError(String),
}

// ---------------------------------------------------------------------------
// ScreenWatchCommand
// ---------------------------------------------------------------------------

/// Commands that can be sent to a running [`ScreenWatcher`] via its command channel.
#[derive(Debug, Clone)]
pub enum ScreenWatchCommand {
    /// Suspend periodic screenshot capture.
    /// The watcher continues running but does not fire interval ticks until resumed.
    /// `capture_now` still works while paused.
    Pause,
    /// Resume periodic screenshot capture. The interval timer resets to zero.
    Resume,
    /// Change the capture interval. Clamped to 2 s – 60 s. Takes effect immediately;
    /// the timer resets to zero.
    SetInterval(Duration),
    /// Stop the watcher. The `run` future completes after this command is processed.
    Stop,
}

// ---------------------------------------------------------------------------
// ScreenWatcher
// ---------------------------------------------------------------------------

/// Drives periodic screenshot capture and forwards results to a caller-supplied channel.
///
/// Construct with [`ScreenWatcher::new`], then:
/// - call [`ScreenWatcher::capture_now`] for on-demand snapshots, or
/// - call [`ScreenWatcher::run`] to start the interval loop.
///
/// The interval loop accepts [`ScreenWatchCommand`]s via a separate channel so
/// callers can pause, resume, change the interval, or stop the watcher without
/// cancelling the task.
pub struct ScreenWatcher {
    interval: Duration,
    screenshot_params: ScreenshotParams,
    paused: bool,
}

impl ScreenWatcher {
    /// Creates a new watcher.  The `interval` in `config` is clamped to 2 s – 60 s.
    pub fn new(config: ScreenWatcherConfig) -> Self {
        Self {
            interval: clamp_interval(config.interval),
            screenshot_params: config.screenshot_params,
            paused: false,
        }
    }

    /// The effective capture interval (after clamping).
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Whether the watcher is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Suspend the interval loop.  Has no effect if already paused.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume the interval loop.  Has no effect if not paused.
    /// The interval timer resets to zero when resumed.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Change the capture interval.  Clamped to 2 s – 60 s.
    pub fn set_interval(&mut self, d: Duration) {
        self.interval = clamp_interval(d);
    }

    /// Capture a single screenshot immediately, regardless of the interval or pause state.
    ///
    /// Returns `Err` if the platform actor fails or produces no screenshot.
    pub async fn capture_now(&self, actor: &mut dyn Actor) -> Result<Screenshot, String> {
        let result = actor
            .perform_actions(
                &[],
                Options {
                    screenshot_params: Some(self.screenshot_params),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        result
            .screenshot
            .ok_or_else(|| "No screenshot produced by actor".to_string())
    }

    /// Run the watcher loop.
    ///
    /// Periodic screenshots are sent to `tx`.  Commands (pause/resume/stop/etc.)
    /// are received from `cmd_rx`.
    ///
    /// The future completes when:
    /// - a [`ScreenWatchCommand::Stop`] is received, or
    /// - `cmd_rx` is closed, or
    /// - `tx` is closed (receiver dropped).
    pub async fn run(
        mut self,
        mut actor: Box<dyn Actor>,
        tx: tokio::sync::mpsc::Sender<ScreenWatchEvent>,
        mut cmd_rx: tokio::sync::mpsc::Receiver<ScreenWatchCommand>,
    ) {
        let mut interval = Self::make_interval(self.interval);

        loop {
            tokio::select! {
                biased; // process commands before firing a tick

                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(ScreenWatchCommand::Pause) => {
                            self.paused = true;
                        }
                        Some(ScreenWatchCommand::Resume) => {
                            self.paused = false;
                            // Reset the timer so the next screenshot fires at now + interval.
                            interval = Self::make_interval(self.interval);
                        }
                        Some(ScreenWatchCommand::SetInterval(d)) => {
                            self.interval = clamp_interval(d);
                            // Reset the timer to use the new interval immediately.
                            interval = Self::make_interval(self.interval);
                        }
                        Some(ScreenWatchCommand::Stop) | None => return,
                    }
                }

                _ = interval.tick() => {
                    if self.paused {
                        continue;
                    }

                    let event = capture_screenshot(self.screenshot_params, &mut *actor).await;
                    if tx.send(event).await.is_err() {
                        // Receiver dropped – stop gracefully.
                        return;
                    }
                }
            }
        }
    }

    fn make_interval(d: Duration) -> tokio::time::Interval {
        let mut interval = tokio::time::interval(d);
        // Skip missed ticks rather than bursting to catch up.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Reset the start time so the first screenshot fires at now + d rather than
        // immediately (which is the default behaviour of `tokio::time::interval`).
        interval.reset_at(tokio::time::Instant::now() + d);
        interval
    }
}

/// Capture a screenshot from `actor` and wrap the result in a [`ScreenWatchEvent`].
async fn capture_screenshot(params: ScreenshotParams, actor: &mut dyn Actor) -> ScreenWatchEvent {
    match actor
        .perform_actions(&[], Options { screenshot_params: Some(params) })
        .await
    {
        Ok(ActionResult { screenshot: Some(screenshot), .. }) => {
            ScreenWatchEvent::Screenshot(screenshot)
        }
        Ok(ActionResult { screenshot: None, .. }) => {
            ScreenWatchEvent::CaptureError("Actor produced no screenshot".to_string())
        }
        Err(e) => ScreenWatchEvent::CaptureError(e),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;

    use super::*;
    use crate::{Action, ActionResult, Options, Platform};

    // A minimal actor that returns a trivial 1×1 screenshot on every call and tracks
    // the total number of `perform_actions` invocations.
    struct TrackingActor {
        call_count: Arc<AtomicUsize>,
    }

    impl TrackingActor {
        fn new(call_count: Arc<AtomicUsize>) -> Self {
            Self { call_count }
        }

        fn screenshot() -> Screenshot {
            Screenshot {
                width: 1,
                height: 1,
                original_width: 1,
                original_height: 1,
                data: vec![0u8],
                mime_type: "image/png".into(),
            }
        }
    }

    #[async_trait]
    impl Actor for TrackingActor {
        fn platform(&self) -> Option<Platform> {
            None
        }

        async fn perform_actions(
            &mut self,
            _actions: &[Action],
            _options: Options,
        ) -> Result<ActionResult, String> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ActionResult {
                screenshot: Some(Self::screenshot()),
                cursor_position: None,
            })
        }
    }

    fn default_params() -> ScreenshotParams {
        ScreenshotParams {
            max_long_edge_px: None,
            max_total_px: None,
            region: None,
        }
    }

    // -----------------------------------------------------------------------
    // Interval clamping
    // -----------------------------------------------------------------------

    #[test]
    fn test_screen_watcher_clamps_interval_below_min() {
        let w = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_millis(500),
            screenshot_params: default_params(),
        });
        assert_eq!(w.interval(), MIN_INTERVAL);
    }

    #[test]
    fn test_screen_watcher_clamps_interval_above_max() {
        let w = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(120),
            screenshot_params: default_params(),
        });
        assert_eq!(w.interval(), MAX_INTERVAL);
    }

    #[test]
    fn test_screen_watcher_accepts_valid_interval() {
        let w = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(10),
            screenshot_params: default_params(),
        });
        assert_eq!(w.interval(), Duration::from_secs(10));
    }

    #[test]
    fn test_set_interval_clamps() {
        let mut w = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });
        w.set_interval(Duration::from_millis(100));
        assert_eq!(w.interval(), MIN_INTERVAL);
        w.set_interval(Duration::from_secs(200));
        assert_eq!(w.interval(), MAX_INTERVAL);
    }

    // -----------------------------------------------------------------------
    // Pause / resume state
    // -----------------------------------------------------------------------

    #[test]
    fn test_pause_resume_state() {
        let mut w = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });
        assert!(!w.is_paused());
        w.pause();
        assert!(w.is_paused());
        w.resume();
        assert!(!w.is_paused());
    }

    // -----------------------------------------------------------------------
    // capture_now works regardless of paused state
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_capture_now_while_paused() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let mut watcher = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });
        watcher.pause();

        let mut actor = TrackingActor::new(call_count.clone());
        let result = watcher.capture_now(&mut actor).await;
        assert!(result.is_ok(), "capture_now should succeed even while paused");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // -----------------------------------------------------------------------
    // Run loop: stops when the event channel is dropped
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_screen_watcher_stops_when_channel_dropped() {
        tokio::time::pause();

        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<ScreenWatchEvent>(10);
        let (_cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<ScreenWatchCommand>(10);

        let call_count = Arc::new(AtomicUsize::new(0));
        let actor = Box::new(TrackingActor::new(call_count.clone()));

        let watcher = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });

        let handle = tokio::spawn(watcher.run(actor, event_tx, cmd_rx));

        // Yield once so the spawned task starts and registers its interval timer at t=0
        // (fires at t=5).  Without this yield the task would not run until after advance(),
        // at which point it would register the timer at t=6+5=11, past our advance window.
        tokio::task::yield_now().await;

        // Drop the receiver – the watcher should detect this on the next send attempt.
        drop(event_rx);

        // Advance time past the interval so the watcher tries to send an event.
        tokio::time::advance(Duration::from_secs(6)).await;

        // Give the spawned task multiple opportunities to run and observe the closed channel.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }

        assert!(
            handle.is_finished(),
            "watcher should have stopped after event channel was dropped"
        );
    }

    // -----------------------------------------------------------------------
    // Run loop: stops on Stop command
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_screen_watcher_stops_on_stop_command() {
        tokio::time::pause();

        let (event_tx, _event_rx) = tokio::sync::mpsc::channel::<ScreenWatchEvent>(10);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<ScreenWatchCommand>(10);

        let call_count = Arc::new(AtomicUsize::new(0));
        let actor = Box::new(TrackingActor::new(call_count.clone()));

        let watcher = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });

        let handle = tokio::spawn(watcher.run(actor, event_tx, cmd_rx));

        cmd_tx.send(ScreenWatchCommand::Stop).await.unwrap();
        tokio::task::yield_now().await;

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("watcher did not stop after Stop command")
            .expect("watcher task panicked");
    }

    // -----------------------------------------------------------------------
    // Run loop: paused watcher does not emit screenshot events
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_screen_watcher_pause_skips_capture() {
        tokio::time::pause();

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ScreenWatchEvent>(10);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<ScreenWatchCommand>(10);

        let call_count = Arc::new(AtomicUsize::new(0));
        let actor = Box::new(TrackingActor::new(call_count.clone()));

        let watcher = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });

        let _handle = tokio::spawn(watcher.run(actor, event_tx, cmd_rx));

        // Pause the watcher before any tick fires.
        cmd_tx.send(ScreenWatchCommand::Pause).await.unwrap();
        // Yield so the run loop processes the Pause command.
        tokio::task::yield_now().await;

        // Advance time well past two intervals – no screenshots should be emitted.
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::task::yield_now().await;

        assert!(
            event_rx.try_recv().is_err(),
            "paused watcher should not emit screenshot events"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "paused watcher should not call perform_actions"
        );
    }

    // -----------------------------------------------------------------------
    // Run loop: resume resets the timer and capture resumes
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_screen_watcher_resume_restarts_timer() {
        tokio::time::pause();

        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ScreenWatchEvent>(10);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<ScreenWatchCommand>(10);

        let call_count = Arc::new(AtomicUsize::new(0));
        let actor = Box::new(TrackingActor::new(call_count.clone()));

        let watcher = ScreenWatcher::new(ScreenWatcherConfig {
            interval: Duration::from_secs(5),
            screenshot_params: default_params(),
        });

        let _handle = tokio::spawn(watcher.run(actor, event_tx, cmd_rx));

        // Pause immediately.
        cmd_tx.send(ScreenWatchCommand::Pause).await.unwrap();
        tokio::task::yield_now().await;

        // No events while paused.
        tokio::time::advance(Duration::from_secs(10)).await;
        tokio::task::yield_now().await;
        assert!(event_rx.try_recv().is_err());

        // Resume – timer resets to 0.
        cmd_tx.send(ScreenWatchCommand::Resume).await.unwrap();
        tokio::task::yield_now().await;

        // Advance just past the interval – one screenshot should be emitted.
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;

        let event = event_rx
            .try_recv()
            .expect("expected a screenshot event after resume");
        assert!(
            matches!(event, ScreenWatchEvent::Screenshot(_)),
            "expected Screenshot event"
        );
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
