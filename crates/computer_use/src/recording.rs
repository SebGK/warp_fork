//! Recording and replay of desktop actions.
//!
//! This module provides:
//! - [`RecordedStep`]: a single captured user action with an optional before-screenshot.
//! - [`RecordingSession`]: an ordered collection of steps produced by a recording session.
//! - [`AutomationLoop`]: drives replay of a `RecordingSession` against a live [`Actor`].

use std::future::Future;

use crate::{Action, Actor, Options, Screenshot, ScreenshotParams};

// ---------------------------------------------------------------------------
// RecordedStep
// ---------------------------------------------------------------------------

/// A single step captured during a desktop recording session.
///
/// `before_screenshot` is taken immediately before the action is performed and may be
/// `None` when screenshots are disabled (e.g. in tests or when `screenshot_params` is
/// not set).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedStep {
    /// Screenshot of the desktop state immediately before this action was taken.
    pub before_screenshot: Option<Screenshot>,
    /// The action that was performed at this step.
    pub action: Action,
}

// ---------------------------------------------------------------------------
// RecordingSession
// ---------------------------------------------------------------------------

/// An ordered collection of [`RecordedStep`]s produced during a recording session.
///
/// Created via [`RecordingSession::new`], populated with [`RecordingSession::add_step`],
/// and frozen by [`RecordingSession::finish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingSession {
    steps: Vec<RecordedStep>,
    /// Screenshot taken after the final step, representing the desktop's end state.
    final_screenshot: Option<Screenshot>,
}

impl RecordingSession {
    /// Creates an empty recording session.
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            final_screenshot: None,
        }
    }

    /// Appends a step to the in-progress recording.
    pub fn add_step(&mut self, step: RecordedStep) {
        self.steps.push(step);
    }

    /// Freezes the session and attaches the optional final screenshot.
    ///
    /// Returns `self` for chaining.
    pub fn finish(mut self, final_screenshot: Option<Screenshot>) -> Self {
        self.final_screenshot = final_screenshot;
        self
    }

    /// Returns the recorded steps in order.
    pub fn steps(&self) -> &[RecordedStep] {
        &self.steps
    }

    /// Returns the final screenshot taken after the last step, if any.
    pub fn final_screenshot(&self) -> Option<&Screenshot> {
        self.final_screenshot.as_ref()
    }

    /// Returns `true` if no steps were recorded.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Returns the number of recorded steps.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}

impl Default for RecordingSession {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// AutomationLoop
// ---------------------------------------------------------------------------

/// Decision returned by the pre-step callback during replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDecision {
    /// Continue and execute the step's action.
    Proceed,
    /// Stop replay immediately without executing the action.
    Cancel,
}

/// Result of running an [`AutomationLoop`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationLoopResult {
    /// All steps completed successfully.
    Completed {
        steps_completed: usize,
        final_screenshot: Option<Screenshot>,
    },
    /// Replay was cancelled by the pre-step callback before a step executed.
    Cancelled { steps_completed: usize },
    /// An error occurred while executing the action at `steps_completed`.
    Error {
        message: String,
        steps_completed: usize,
    },
}

/// Drives replay of a [`RecordingSession`] against a live [`Actor`].
///
/// For each step, the loop:
/// 1. Captures an optional pre-step screenshot.
/// 2. Calls the caller-supplied `on_pre_step` callback with the step index and screenshot.
/// 3. Executes the action if the callback returns [`StepDecision::Proceed`].
/// 4. Stops early if the callback returns [`StepDecision::Cancel`].
///
/// After all steps succeed, a final screenshot is captured and returned.
pub struct AutomationLoop<'a> {
    actor: &'a mut dyn Actor,
    session: &'a RecordingSession,
    /// Screenshot parameters used for both per-step and final screenshots.
    /// Pass `None` to skip all screenshots.
    screenshot_params: Option<ScreenshotParams>,
}

impl<'a> AutomationLoop<'a> {
    /// Creates a new automation loop.
    pub fn new(
        actor: &'a mut dyn Actor,
        session: &'a RecordingSession,
        screenshot_params: Option<ScreenshotParams>,
    ) -> Self {
        Self {
            actor,
            session,
            screenshot_params,
        }
    }

    /// Runs the automation loop to completion.
    ///
    /// `on_pre_step(step_index, pre_step_screenshot)` is called before each action.
    /// Returning [`StepDecision::Cancel`] stops replay and returns
    /// [`AutomationLoopResult::Cancelled`].
    pub async fn run<F, Fut>(&mut self, on_pre_step: F) -> Result<AutomationLoopResult, String>
    where
        F: Fn(usize, Option<Screenshot>) -> Fut,
        Fut: Future<Output = StepDecision>,
    {
        for (index, step) in self.session.steps().iter().enumerate() {
            // Capture a pre-step screenshot for the callback.
            let pre_screenshot = if let Some(params) = self.screenshot_params {
                let result = self
                    .actor
                    .perform_actions(
                        &[],
                        Options {
                            screenshot_params: Some(params),
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                result.screenshot
            } else {
                None
            };

            let decision = on_pre_step(index, pre_screenshot).await;
            match decision {
                StepDecision::Cancel => {
                    return Ok(AutomationLoopResult::Cancelled {
                        steps_completed: index,
                    });
                }
                StepDecision::Proceed => {}
            }

            // Execute the step's action. We do not capture a screenshot here to avoid
            // a redundant capture; the next iteration's pre-step screenshot covers it.
            let result = self
                .actor
                .perform_actions(
                    &[step.action.clone()],
                    Options {
                        screenshot_params: None,
                    },
                )
                .await;

            if let Err(message) = result {
                return Ok(AutomationLoopResult::Error {
                    message,
                    steps_completed: index,
                });
            }
        }

        // Capture the final screenshot after all steps.
        let final_screenshot = if let Some(params) = self.screenshot_params {
            let result = self
                .actor
                .perform_actions(
                    &[],
                    Options {
                        screenshot_params: Some(params),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            result.screenshot
        } else {
            None
        };

        Ok(AutomationLoopResult::Completed {
            steps_completed: self.session.step_count(),
            final_screenshot,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Action, ActionResult, Options};
    use async_trait::async_trait;
    use pathfinder_geometry::vector::Vector2I;

    // A minimal actor that always succeeds and never produces screenshots.
    struct TestActor;

    #[async_trait]
    impl Actor for TestActor {
        fn platform(&self) -> Option<crate::Platform> {
            None
        }

        async fn perform_actions(
            &mut self,
            _actions: &[Action],
            _options: Options,
        ) -> Result<ActionResult, String> {
            Ok(ActionResult {
                screenshot: None,
                cursor_position: None,
            })
        }
    }

    fn dummy_action() -> Action {
        Action::MouseMove {
            to: Vector2I::new(0, 0),
        }
    }

    // -----------------------------------------------------------------------
    // RecordingSession
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_recording_session() {
        let session = RecordingSession::new();
        assert!(session.is_empty());
        assert_eq!(session.step_count(), 0);
        assert!(session.steps().is_empty());
        assert!(session.final_screenshot().is_none());
    }

    #[test]
    fn test_add_steps() {
        let mut session = RecordingSession::new();
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        assert_eq!(session.step_count(), 2);
        assert!(!session.is_empty());

        let session = session.finish(None);
        assert_eq!(session.step_count(), 2);
        assert!(session.final_screenshot().is_none());
    }

    #[test]
    fn test_is_empty() {
        let mut session = RecordingSession::new();
        assert!(session.is_empty());
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        assert!(!session.is_empty());
    }

    // -----------------------------------------------------------------------
    // AutomationLoop
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_automation_loop_completes() {
        let mut actor = TestActor;
        let mut session = RecordingSession::new();
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        let session = session.finish(None);

        let mut automation = AutomationLoop::new(&mut actor, &session, None);
        let result = automation
            .run(|_index, _screenshot| async { StepDecision::Proceed })
            .await
            .unwrap();

        assert_eq!(
            result,
            AutomationLoopResult::Completed {
                steps_completed: 2,
                final_screenshot: None,
            }
        );
    }

    #[tokio::test]
    async fn test_automation_loop_cancel() {
        let mut actor = TestActor;
        let mut session = RecordingSession::new();
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        session.add_step(RecordedStep {
            before_screenshot: None,
            action: dummy_action(),
        });
        let session = session.finish(None);

        let mut automation = AutomationLoop::new(&mut actor, &session, None);
        // Cancel at step index 1 (the second step).
        let result = automation
            .run(|index, _screenshot| async move {
                if index == 1 {
                    StepDecision::Cancel
                } else {
                    StepDecision::Proceed
                }
            })
            .await
            .unwrap();

        assert_eq!(
            result,
            AutomationLoopResult::Cancelled { steps_completed: 1 }
        );
    }

    #[tokio::test]
    async fn test_automation_loop_empty_session() {
        let mut actor = TestActor;
        let session = RecordingSession::new().finish(None);

        let mut automation = AutomationLoop::new(&mut actor, &session, None);
        let result = automation
            .run(|_index, _screenshot| async { StepDecision::Proceed })
            .await
            .unwrap();

        assert_eq!(
            result,
            AutomationLoopResult::Completed {
                steps_completed: 0,
                final_screenshot: None,
            }
        );
    }
}
