//! Stage 1 input tests — synthetic-event dispatch through the real event
//! path (Stage 1 authorization, item 4).
//!
//! These verify the input state machine and the application loop. They do
//! NOT verify physical hardware input: pushing an event dispatches it, but
//! does not update SDL's device state. GUI/hardware evidence is separate
//! (see docs/TECH-ARCHITECTURE §6, Stage 1 record).

mod common;
#[path = "common/host_alloc.rs"]
mod host_alloc;

use std::time::Duration;

use engine_shell::{InputSnapshot, QuitReason, ShellApp};
use sdl3::event::{Event, WindowEvent};
use sdl3::keyboard::{Keycode, Mod, Scancode};
use sdl3::mouse::{MouseButton, MouseState};

fn lock() -> std::sync::MutexGuard<'static, ()> {
    common::sdl_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn scancode_for(key: Keycode) -> Scancode {
    match key {
        Keycode::W => Scancode::W,
        Keycode::A => Scancode::A,
        Keycode::S => Scancode::S,
        Keycode::D => Scancode::D,
        Keycode::Escape => Scancode::Escape,
        _ => Scancode::Unknown,
    }
}

fn key_down(key: Keycode, repeat: bool) -> Event {
    Event::KeyDown {
        timestamp: 0,
        window_id: 0,
        keycode: Some(key),
        scancode: Some(scancode_for(key)),
        keymod: Mod::empty(),
        repeat,
        which: 0,
        raw: 0,
    }
}

fn key_up(key: Keycode) -> Event {
    Event::KeyUp {
        timestamp: 0,
        window_id: 0,
        keycode: Some(key),
        scancode: Some(scancode_for(key)),
        keymod: Mod::empty(),
        repeat: false,
        which: 0,
        raw: 0,
    }
}

fn window_event(win_event: WindowEvent) -> Event {
    Event::Window {
        timestamp: 0,
        window_id: 0,
        win_event,
    }
}

fn motion(xrel: f32, yrel: f32) -> Event {
    Event::MouseMotion {
        timestamp: 0,
        window_id: 0,
        which: 0,
        mousestate: MouseState::from_sdl_state(0),
        x: 0.0,
        y: 0.0,
        xrel,
        yrel,
    }
}

fn button_down(button: MouseButton) -> Event {
    Event::MouseButtonDown {
        timestamp: 0,
        window_id: 0,
        which: 0,
        mouse_btn: button,
        clicks: 1,
        x: 0.0,
        y: 0.0,
    }
}

fn button_up(button: MouseButton) -> Event {
    Event::MouseButtonUp {
        timestamp: 0,
        window_id: 0,
        which: 0,
        mouse_btn: button,
        clicks: 1,
        x: 0.0,
        y: 0.0,
    }
}

/// Push one synthetic event, then pump one frame.
fn dispatch(app: &mut ShellApp, event: Event) -> InputSnapshot {
    app.push_event(event).expect("synthetic push");
    app.frame()
}

#[test]
fn wasd_axes_and_key_edges() {
    let _guard = lock();
    let mut app = common::dummy_app("wasd");

    let snap = dispatch(&mut app, key_down(Keycode::W, false));
    assert_eq!(snap.move_forward, 1);
    assert_eq!(snap.keys_pressed, vec![Keycode::W]);
    assert!(app.is_held(Keycode::W));

    let snap = dispatch(&mut app, key_down(Keycode::S, false));
    assert_eq!(snap.move_forward, 0, "W and S cancel");

    let snap = dispatch(&mut app, key_up(Keycode::W));
    assert_eq!(snap.move_forward, -1);
    assert_eq!(snap.keys_released, vec![Keycode::W]);

    let snap = dispatch(&mut app, key_down(Keycode::D, false));
    assert_eq!(snap.move_strafe, 1);

    let snap = dispatch(&mut app, key_down(Keycode::A, false));
    assert_eq!(snap.move_strafe, 0, "D and A cancel");
}

#[test]
fn key_repeats_are_counted_not_repressed() {
    let _guard = lock();
    let mut app = common::dummy_app("repeat");

    dispatch(&mut app, key_down(Keycode::W, false));
    let snap = dispatch(&mut app, key_down(Keycode::W, true));
    assert_eq!(snap.key_repeats, 1);
    assert!(
        snap.keys_pressed.is_empty(),
        "a repeat is not a fresh press"
    );
    assert_eq!(snap.move_forward, 1, "the key remains held");
}

#[test]
fn look_deltas_require_relative_capture_and_do_not_accumulate() {
    let _guard = lock();
    let mut app = common::dummy_app("look");

    let snap = dispatch(&mut app, motion(9.0, 9.0));
    assert_eq!(
        snap.look_delta,
        (0.0, 0.0),
        "deltas are ignored while uncaptured"
    );

    app.set_relative_mouse(true);
    assert!(app.relative_mouse());
    let snap = dispatch(&mut app, motion(3.5, -2.0));
    assert_eq!(snap.look_delta, (3.5, -2.0));
    let snap = dispatch(&mut app, motion(1.0, 1.0));
    assert_eq!(
        snap.look_delta,
        (1.0, 1.0),
        "deltas are consumed per snapshot"
    );

    app.set_relative_mouse(false);
    let snap = dispatch(&mut app, motion(9.0, 9.0));
    assert_eq!(
        snap.look_delta,
        (0.0, 0.0),
        "capture release gates deltas again"
    );
}

#[test]
fn focus_loss_clears_held_keys_and_pending_deltas() {
    let _guard = lock();
    let mut app = common::dummy_app("focus-loss");
    app.set_relative_mouse(true);

    let snap = dispatch(&mut app, key_down(Keycode::W, false));
    assert_eq!(snap.move_forward, 1);

    app.push_event(motion(2.0, 2.0)).expect("push motion");
    app.push_event(window_event(WindowEvent::FocusLost))
        .expect("push focus loss");
    let snap = app.frame();
    assert!(snap.focus_lost);
    assert_eq!(
        snap.look_delta,
        (0.0, 0.0),
        "pending deltas are discarded on focus loss"
    );
    assert!(
        !app.is_held(Keycode::W),
        "held keys are cleared on focus loss"
    );
    let snap = app.frame();
    assert_eq!(snap.move_forward, 0);
}

#[test]
fn focus_regain_requires_a_fresh_press() {
    let _guard = lock();
    let mut app = common::dummy_app("focus-regain");

    dispatch(&mut app, key_down(Keycode::W, false));
    dispatch(&mut app, window_event(WindowEvent::FocusLost));
    let snap = dispatch(&mut app, window_event(WindowEvent::FocusGained));
    assert!(snap.focus_gained);
    assert_eq!(
        snap.move_forward, 0,
        "focus regain does not resurrect held keys"
    );

    let snap = dispatch(&mut app, key_down(Keycode::W, false));
    assert_eq!(snap.move_forward, 1, "a fresh press works after regain");
}

#[test]
fn resize_is_reported_once() {
    let _guard = lock();
    let mut app = common::dummy_app("resize");

    let snap = dispatch(&mut app, window_event(WindowEvent::Resized(1024, 768)));
    assert_eq!(snap.resized, Some((1024, 768)));
    let snap = app.frame();
    assert_eq!(snap.resized, None, "resize is reported once");
}

#[test]
fn close_request_quits_stickily() {
    let _guard = lock();
    let mut app = common::dummy_app("close");

    let snap = dispatch(&mut app, window_event(WindowEvent::CloseRequested));
    assert!(snap.quit_requested);
    let snap = app.frame();
    assert!(snap.quit_requested, "quit is sticky once requested");
}

#[test]
fn sdl_quit_event_quits() {
    let _guard = lock();
    let mut app = common::dummy_app("sdl-quit");

    let snap = dispatch(&mut app, Event::Quit { timestamp: 0 });
    assert!(snap.quit_requested);
}

#[test]
fn mouse_button_edges_are_reported() {
    let _guard = lock();
    let mut app = common::dummy_app("mouse-edges");

    let snap = dispatch(&mut app, button_down(MouseButton::Left));
    assert_eq!(snap.mouse_pressed, vec![MouseButton::Left]);
    let snap = dispatch(&mut app, button_up(MouseButton::Left));
    assert_eq!(snap.mouse_released, vec![MouseButton::Left]);
}

#[test]
fn paced_loop_runs_and_stops_via_callback() {
    let _guard = lock();
    let mut app = common::dummy_app("loop-callback");
    let mut frames = 0;
    let summary = app.run(|_, _| {
        frames += 1;
        if frames >= 10 {
            engine_shell::LoopControl::Quit
        } else {
            engine_shell::LoopControl::Continue
        }
    });
    assert_eq!(summary.frames, 10);
    assert_eq!(summary.quit_reason, QuitReason::Callback);
    assert_eq!(summary.frame_stats.count, 10);
    assert!(
        summary.wall_duration >= Duration::from_millis(150),
        "pacing must actually sleep: {:?}",
        summary.wall_duration
    );
}

#[test]
fn paced_loop_stops_on_close_request() {
    let _guard = lock();
    let mut app = common::dummy_app("loop-close");
    app.push_event(window_event(WindowEvent::CloseRequested))
        .expect("push close");
    let summary = app.run(|_, _| engine_shell::LoopControl::Continue);
    assert_eq!(summary.quit_reason, QuitReason::Requested);
    assert_eq!(summary.frames, 1, "the close is honored by the first frame");
}

#[test]
fn close_request_is_reported_as_requested_when_callback_quits() {
    // Stage 1 corrective pass, review finding 3: when the frame callback
    // quits upon observing a close request, the reported cause must be
    // QuitReason::Requested, not Callback.
    let _guard = lock();
    let mut app = common::dummy_app("quit-reason");
    app.push_event(window_event(WindowEvent::CloseRequested))
        .expect("push close");
    let summary = app.run(|_, snap| {
        if snap.quit_requested {
            engine_shell::LoopControl::Quit
        } else {
            engine_shell::LoopControl::Continue
        }
    });
    assert_eq!(summary.quit_reason, QuitReason::Requested);
}

#[test]
fn focus_transitions_are_recorded_in_diagnostics() {
    // Stage 1 corrective pass, review finding 4: focus loss/regain are
    // recorded through the shell's diagnostics; core input behavior is
    // unchanged. SDL itself may enqueue window events (e.g. an initial
    // focus-gain) when a window is created, so the queue is drained
    // BEFORE diagnostics are enabled — only the synthetic transitions
    // below are then recorded.
    let _guard = lock();
    let mut app = common::dummy_app("focus-diag");
    app.frame(); // drain SDL-generated window events
    app.set_diag_level(engine_shell::Level::Info);
    app.push_event(window_event(WindowEvent::FocusLost))
        .expect("push focus lost");
    app.push_event(window_event(WindowEvent::FocusGained))
        .expect("push focus gained");
    let snap = app.frame();
    assert!(snap.focus_lost);
    assert!(snap.focus_gained);
    let records = app.diagnostics().records();
    assert_eq!(records.len(), 2);
    assert!(records[0].message.contains("focus lost"));
    assert!(records[1].message.contains("focus regained"));
}

/// Counts only Rust allocations inside the frame pump. SDL event injection,
/// snapshot checks and reporting all run outside the measuring window.
#[test]
fn warmed_input_frame_allocation_census() {
    let _guard = lock();
    let mut app = common::dummy_app("input allocation census");
    let mut snapshot = InputSnapshot::default();
    for (key_edge, cycles) in [(false, 0), (true, 0), (false, 1), (false, 32)] {
        let mut totals = (0, 0);
        // A press/release pair warms held-key membership and both edge directions.
        for frame in 0..34 {
            if key_edge {
                let event = if frame % 2 == 0 {
                    key_down(Keycode::W, false)
                } else {
                    key_up(Keycode::W)
                };
                app.push_event(event).unwrap();
            }
            for _ in 0..cycles {
                for event in [
                    key_down(Keycode::W, false),
                    button_down(MouseButton::Left),
                    key_up(Keycode::W),
                    button_up(MouseButton::Left),
                ] {
                    app.push_event(event).unwrap();
                }
            }
            let scope = host_alloc::HostAllocScope::new();
            app.frame_into(&mut snapshot);
            let (count, bytes) = scope.totals();
            drop(scope);
            if frame >= 2 {
                totals.0 += count;
                totals.1 += bytes;
            }
            let pressed = usize::from(key_edge && frame % 2 == 0);
            let released = usize::from(key_edge && frame % 2 == 1);
            assert_eq!(snapshot.keys_pressed.len(), cycles + pressed);
            assert_eq!(snapshot.keys_released.len(), cycles + released);
            assert_eq!(snapshot.mouse_pressed.len(), cycles);
            assert_eq!(snapshot.mouse_released.len(), cycles);
        }
        eprintln!(
            "single key edge={key_edge}, mixed cycles={cycles}: 32 frames, {totals:?} (allocations, requested bytes)"
        );
        assert_eq!(totals, (0, 0));
    }
}

#[test]
fn reusing_an_edited_snapshot_preserves_event_order_and_frame_boundaries() {
    let _guard = lock();
    let mut app = common::dummy_app("reusable input");
    app.frame();
    app.set_relative_mouse(true);
    app.push_event(key_down(Keycode::W, false)).unwrap();
    let retained = app.frame();
    let mut snapshot = retained.clone();

    // Application mappers can change any public field between polls.
    snapshot.keys_pressed.push(Keycode::Escape);
    snapshot.mouse_released.push(MouseButton::Right);
    snapshot.quit_requested = true;
    for event in [
        key_down(Keycode::D, false),
        key_down(Keycode::A, false),
        key_down(Keycode::A, true),
        key_up(Keycode::D),
        key_up(Keycode::W),
        button_down(MouseButton::Right),
        button_down(MouseButton::Left),
        button_up(MouseButton::Left),
        button_up(MouseButton::Right),
        motion(5.0, -1.0),
        window_event(WindowEvent::FocusLost),
        window_event(WindowEvent::FocusGained),
        window_event(WindowEvent::Minimized),
        window_event(WindowEvent::Restored),
        window_event(WindowEvent::Resized(321, 123)),
        motion(2.0, -3.0),
    ] {
        app.push_event(event).unwrap();
    }
    app.frame_into(&mut snapshot);
    assert_eq!(
        snapshot,
        InputSnapshot {
            keys_pressed: vec![Keycode::D, Keycode::A],
            keys_released: vec![Keycode::D, Keycode::W],
            key_repeats: 1,
            mouse_pressed: vec![MouseButton::Right, MouseButton::Left],
            mouse_released: vec![MouseButton::Left, MouseButton::Right],
            look_delta: (2.0, -3.0),
            resized: Some((321, 123)),
            focus_lost: true,
            focus_gained: true,
            minimized: true,
            restored: true,
            ..InputSnapshot::default()
        }
    );
    app.push_event(Event::Quit { timestamp: 0 }).unwrap();
    for _ in 0..2 {
        app.frame_into(&mut snapshot);
        assert_eq!(
            snapshot,
            InputSnapshot {
                quit_requested: true,
                ..InputSnapshot::default()
            }
        );
    }
    assert_eq!(retained.keys_pressed, [Keycode::W]);
    assert_eq!(retained.move_forward, 1);
}
