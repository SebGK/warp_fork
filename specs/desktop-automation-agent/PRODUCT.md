# Desktop Automation Agent

Figma: none provided

## Summary

Users can record a demonstration of a desktop task in Warp (screen capture + mouse/keyboard actions), and then have the AI agent repeat that task automatically using computer use. This closes the loop between "show the agent what to do" and "have the agent do it again," without requiring the user to write code or manual scripts.

## Behavior

### Desktop Recording

1. A user can initiate a desktop recording session from within a Warp agent conversation by asking the AI to automate a task they want to demonstrate, or by explicitly requesting a recording.

2. When a recording session is active, Warp captures each user action (mouse click, mouse move, keyboard input, scroll) together with a screenshot taken immediately before that action. Together, each such (before-screenshot, action) pair is a **recorded step**.

3. A screenshot is also taken at the end of the last step so there is always a "final state" screenshot that shows what the desktop looked like after the demonstration was complete.

4. The user signals the end of the recording explicitly (e.g. through the Warp UI). At that point the recording is stopped and the captured steps are frozen.

5. A recording that contains zero steps is valid but produces no automation when replayed.

6. A recording is serializable and can be stored (e.g. in a Warp conversation artifact or on disk) so it can be replayed later or inspected.

7. After a recording is complete, the AI agent receives the full set of recorded steps as context — including all before-screenshots and the final-state screenshot — so it can reason about what was demonstrated.

### Automation Replay

8. After receiving a recording, the AI agent can request that Warp replay the recorded actions on the live desktop.

9. Replay executes steps in the original order. Before each step, Warp takes a fresh screenshot and passes it to the agent so the agent can verify the desktop is in the expected state before proceeding.

10. If the desktop state before a step differs from what was recorded (for example, a window moved, a dialog appeared, or the wrong application is focused), the agent must decide whether to proceed, adjust coordinates, or stop. The agent communicates its decision back before Warp executes the action.

11. After all steps complete, Warp takes a final screenshot and returns it to the agent as confirmation.

12. **Risky action confirmation**: any action that the agent or the system judges as potentially destructive (e.g. clicking a "Delete" or "Confirm" button, pressing Enter when a destructive dialog is open) presents a confirmation request to the user before executing. The user can approve or cancel the individual action, and the replay continues or stops accordingly.

13. If the user cancels replay at any point, replay stops immediately after the current in-flight action finishes. No further actions are executed. The agent receives a result indicating how many steps completed before cancellation.

14. If an action fails (e.g. a coordinate is off-screen, the OS rejects the input), replay stops and the agent receives an error result with the index of the failing step.

15. Replay does not loop or retry steps on its own. If the agent wants to retry, it must explicitly send a new replay request.

### Safety and Permissions

16. Desktop capture and action replay are only available on supported platforms (macOS, Windows, and Linux X11/Wayland). On unsupported platforms, any recording or replay request returns an error immediately, with no UI shown.

17. The first time a recording or replay is initiated in a session, Warp shows a one-time permission prompt explaining that it will observe the screen and perform mouse/keyboard actions on the user's behalf. The user must explicitly approve before capture or replay begins.

18. Screenshot data captured during a recording or replay session is transmitted to the AI model only if the user has approved the permission prompt (invariant 17) and only for the duration of the active agent task. No screenshot is stored beyond the current agent conversation unless the user explicitly saves the recording.

19. The permission prompt cannot be bypassed programmatically. It must be dismissed by a human interaction.

### Edge Cases

20. If the user closes Warp or the agent session ends while a recording is active, the partial recording is discarded. No partial data is sent to the agent.

21. If the user closes Warp or the agent session ends while replay is active, all pending actions are cancelled. Already-executed actions are not reversed.

22. On platforms where screen capture requires additional OS-level permission (e.g. macOS Screen Recording permission), Warp prompts the user to grant that permission before starting any session. If permission is denied, a clear error is shown and no capture occurs.

23. If the screen resolution or scaling factor changes between when steps were recorded and when they are replayed, the coordinate values in the recorded steps are unchanged but the agent is informed of the current screen dimensions so it can reason about any mismatch.

24. A recording session and a replay session cannot run simultaneously. Attempting to start one while the other is active returns an error to the agent.

25. If the agent requests a replay with an empty recording (zero steps), replay completes immediately and returns a success result with zero steps completed and a fresh screenshot of the current desktop state.
