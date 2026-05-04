# Screen-Watching Chat

Figma: none provided

## Summary

Warp can continuously monitor the user's screen and interact with them through a small floating chat popup window. The AI model observes periodic screenshots, proactively surfaces observations or assistance, and responds to user messages with full awareness of what is currently on screen.

## Goals / Non-goals

**Goals:**
- Continuous, passive screen observation that can run alongside any desktop work.
- A lightweight floating chat popup the user can interact with without switching away from their work.
- The AI model proactively engages when it detects something noteworthy on screen.
- The user can ask questions and the AI answers with the current screen as context.

**Non-goals:**
- Replacing the existing agent conversation panel — the popup is a companion surface, not a substitute.
- Performing automated computer-use actions in this mode (recording and replay remain a separate flow).
- Streaming continuous video — only periodic still screenshots are captured.

## Behavior

### Activation

1. The user can start a Screen-Watch session from within any active Warp agent conversation by sending a message such as "Watch my screen and help me" or by clicking a dedicated "Start Screen Watch" affordance in the agent input area. The agent responds by launching a screen-watch session.

2. Once a session is active, a floating chat popup window appears. The popup is visible on top of all other windows.

3. A screen-watch session can only be started when computer use is supported on the current platform (macOS, Windows, Linux X11/Wayland). On unsupported platforms the attempt fails and the user sees a clear error message.

4. Only one screen-watch session can be active at a time. Attempting to start a second session while one is already running shows an error and leaves the existing session unchanged.

5. If a desktop recording or replay session (from the Desktop Automation Agent feature) is already active, starting a screen-watch session returns an error. The two modes cannot run simultaneously.

### Permission prompt

6. The first time a screen-watch session is started in a given Warp installation, Warp presents a one-time permission dialog explaining that it will periodically capture screenshots of the entire screen and send them to the AI model, along with any messages the user types. The user must explicitly approve before any screenshots are taken.

7. The permission dialog cannot be dismissed programmatically. It requires a human gesture (clicking "Allow" or "Deny").

8. If the user denies the permission dialog, no session starts and no screenshots are taken. The user sees a short message confirming that the session was not started.

9. On platforms that require an OS-level screen-recording permission (e.g. macOS), Warp first prompts the user to grant that OS permission if it has not already been granted. If the OS permission is denied, a clear error is shown and the session does not start.

10. After the initial approval, subsequent sessions in the same installation do not show the permission dialog again. The user can revoke consent at any time from Warp's Privacy settings, which will both stop any active session and require re-approval the next time a session is started.

### Screenshot interval

11. While a session is active, Warp captures a screenshot of the entire screen at a configurable interval. The default interval is 5 seconds. The user can adjust the interval from the chat popup (e.g., "Check every 10 seconds") or from Warp's Privacy settings.

12. The minimum configurable interval is 2 seconds. The maximum is 60 seconds. Values outside this range are clamped silently.

13. A screenshot is also taken immediately when the user sends a message from the popup, so the AI always has the freshest possible view when forming its reply.

14. Screenshots are not stored on disk. They are held in memory only for the duration of the current AI model turn, then discarded. No screenshot is written to a file unless the user explicitly requests it.

15. Screenshot data is transmitted to the AI model only while the session is active and only if the user has approved the permission dialog (invariant 6). It is not retained by Warp beyond the current agent turn.

### Floating chat popup

16. The popup appears as a small, always-on-top window anchored by default to the bottom-right corner of the primary display. The user can drag it to any position on screen; the last position persists across restarts.

17. The popup consists of:
    - A compact conversation thread showing the most recent messages from both the AI model and the user.
    - A single-line text input field at the bottom.
    - A "Send" button (or Enter to send).
    - A status indicator showing whether the session is active, paused, or stopped.
    - A "Pause" / "Resume" button to temporarily suspend screenshot capture without closing the session.
    - A "Stop" button (or close affordance) to end the session.

18. The popup is always on top of other windows but does not steal focus unless the user clicks into the text input or the popup itself.

19. When the user clicks anywhere outside the popup, keyboard focus returns to the previously focused application. The popup remains visible.

20. The popup can be minimized to a small icon/badge in the corner of the screen. Clicking the icon restores it. New AI messages while minimized cause the icon to display an unread badge.

21. Keyboard shortcut: a global hotkey (user-configurable, default unset) can be used to show/hide the popup or to move focus to the input field when the popup is already visible.

### AI observations and responses

22. After each screenshot is captured, the AI model is given the screenshot as context. The model decides whether to send a proactive observation to the user. The model must not send a message if the screen content has not meaningfully changed since the last turn.

23. The model's proactive messages appear in the popup conversation thread. Each message indicates the approximate time it was generated.

24. When the user sends a message from the popup, Warp captures a fresh screenshot, attaches it to the user message, and sends both to the AI model. The model's response is shown in the popup thread.

25. The popup conversation thread is distinct from the main Warp agent conversation. Messages sent through the popup do not appear in the main agent panel, and vice versa. The popup maintains its own conversation history for the duration of the session.

26. If the AI model requires more than a few seconds to respond, a typing indicator is shown in the popup thread while the response is in flight.

27. If the AI response fails (network error, model error, rate limit), the popup displays a short error message inline in the thread. The session remains active and the next scheduled screenshot will be processed normally.

### Pause and resume

28. The user can pause screenshot capture at any time by clicking "Pause" in the popup or by sending a message such as "Pause". While paused:
    - No new screenshots are taken.
    - The status indicator clearly shows "Paused".
    - The user can still send messages; a fresh screenshot is taken when the user sends a message, even while paused.

29. The user can resume screenshot capture by clicking "Resume" or by sending a message such as "Resume". The interval timer restarts from zero when resuming.

### Stopping the session

30. The user can stop the session at any time by clicking "Stop" in the popup, closing the popup window, or sending a message such as "Stop watching my screen."

31. When a session stops:
    - No further screenshots are taken.
    - The popup closes (or, if the user closed it to trigger the stop, it is already gone).
    - The main Warp agent conversation receives a system note that the screen-watch session has ended.
    - All in-memory screenshot data is cleared immediately.

32. If Warp is quit while a session is active, the session is stopped cleanly. No partial data is persisted.

33. If the agent conversation that started the session is closed while the session is active, the session is stopped and the popup closes.

### Privacy and data handling

34. The popup displays a persistent, non-dismissible indicator (e.g., a colored border or badge) that clearly shows a screen-watch session is active. This indicator disappears when the session is stopped.

35. The user can view a log of which screenshots were taken (timestamps only, not image content) from the popup menu or from Warp's Privacy settings page.

36. Warp does not capture screenshots of itself (i.e., the Warp window and the floating popup are excluded from the captured region where technically feasible). If exclusion is not possible on a given platform, the spec note is surfaced to the user in the permission dialog.

### Edge cases

37. If the screen resolution or the number of connected displays changes while a session is active, the next screenshot reflects the new configuration without requiring the user to restart the session.

38. If the primary display is disconnected while a session is active, screenshot capture pauses automatically and the popup shows a "Display unavailable" status. Capture resumes automatically when a display is reconnected.

39. If the device is locked (screensaver active, fast user switch, lid close), screenshot capture pauses automatically. Capture resumes when the session is unlocked. The model is not sent a screenshot of the lock screen.

40. If the AI model is unavailable (Warp is offline, the server is unreachable) when a screenshot interval fires, the capture is skipped for that interval. The popup shows an offline indicator. Capture continues as normal once connectivity is restored.

41. Very large screenshots (e.g., 8K display) are downscaled before being sent to the model, using the same constraints as `ScreenshotParams` in the existing computer use infrastructure. The user is not notified of downscaling unless it causes visible quality loss.

42. If a screenshot interval fires while the previous AI turn has not yet returned a response, the new screenshot is queued. The model is not sent two screenshots simultaneously; the queued screenshot is sent as soon as the previous turn completes.

43. Multi-display setups: by default Warp captures only the primary display. The user can switch capture to any connected display from the popup menu.
