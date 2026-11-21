//! Startup configuration for the four interactive player demos.

use engine_shell::{InputSnapshot, Keycode, MouseButton};

#[derive(Clone, Copy)]
pub(crate) enum Action {
    Forward,
    Back,
    Left,
    Right,
    Sprint,
    Jump,
    Interact,
    Camera,
    Reset,
    Precision,
    Spread,
    Cycle,
    Overlay,
    Quit,
}

use Action::*;

const KEYS: [(Action, &str, Keycode); 14] = [
    (Forward, "forward", Keycode::W),
    (Back, "back", Keycode::S),
    (Left, "left", Keycode::A),
    (Right, "right", Keycode::D),
    (Sprint, "sprint", Keycode::LShift),
    (Jump, "jump", Keycode::Space),
    (Interact, "interact", Keycode::E),
    (Camera, "camera", Keycode::C),
    (Reset, "reset", Keycode::R),
    (Precision, "precision", Keycode::_1),
    (Spread, "spread", Keycode::_2),
    (Cycle, "cycle", Keycode::Q),
    (Overlay, "overlay", Keycode::O),
    (Quit, "quit", Keycode::Escape),
];

#[derive(Clone, Copy, PartialEq)]
enum FireBinding {
    Mouse(MouseButton),
    Key(Keycode),
}

pub(crate) struct PlayerControls {
    keys: [Keycode; KEYS.len()],
    fire: FireBinding,
    sensitivity: f32,
}

impl PlayerControls {
    pub(crate) fn load() -> Result<Self, String> {
        fn setting(name: &str) -> Result<Option<String>, String> {
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(_) => Err(format!("{name} must be valid text")),
            }
        }
        Self::parse(
            setting("POSTRUST_BINDINGS")?.as_deref(),
            setting("POSTRUST_LOOK_SENSITIVITY")?.as_deref(),
        )
    }

    fn parse(bindings: Option<&str>, sensitivity: Option<&str>) -> Result<Self, String> {
        let sensitivity = sensitivity
            .unwrap_or("1")
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0 && *v <= 100.0)
            .ok_or("POSTRUST_LOOK_SENSITIVITY must be greater than zero and at most 100")?;
        let mut controls = Self {
            keys: KEYS.map(|(_, _, key)| key),
            fire: FireBinding::Mouse(MouseButton::Left),
            sensitivity,
        };
        if let Some(raw) = bindings {
            for assignment in raw.split(',') {
                let (name, value) = assignment.split_once('=').ok_or_else(|| {
                    format!("expected action=key in POSTRUST_BINDINGS, got {assignment:?}")
                })?;
                let (name, value) = (name.trim(), value.trim());
                let key = || {
                    Keycode::from_name(value)
                        .ok_or_else(|| format!("unknown SDL key {value:?} for {name}"))
                };
                if name.eq_ignore_ascii_case("fire") {
                    controls.fire = match value.to_ascii_lowercase().as_str() {
                        "mouse1" => FireBinding::Mouse(MouseButton::Left),
                        "mouse2" => FireBinding::Mouse(MouseButton::Right),
                        "mouse3" => FireBinding::Mouse(MouseButton::Middle),
                        "mouse4" => FireBinding::Mouse(MouseButton::X1),
                        "mouse5" => FireBinding::Mouse(MouseButton::X2),
                        _ => FireBinding::Key(key()?),
                    };
                } else if let Some((action, _, _)) =
                    KEYS.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(name))
                {
                    controls.keys[*action as usize] = key()?;
                } else {
                    return Err(format!(
                        "unknown player action {name:?} in POSTRUST_BINDINGS"
                    ));
                }
            }
            for (i, &key) in controls.keys.iter().enumerate() {
                if controls.keys[..i].contains(&key) || controls.fire == FireBinding::Key(key) {
                    return Err(format!("key {key} is bound to multiple player actions"));
                }
            }
        }
        Ok(controls)
    }

    pub(crate) fn key(&self, action: Action) -> Keycode {
        self.keys[action as usize]
    }

    /// Translate physical bindings to the logical keys used by the existing
    /// demo mappers. Reuse the event vectors returned by the SDL poll.
    pub(crate) fn map_frame(
        &self,
        snapshot: &mut InputSnapshot,
        held: impl Fn(Keycode) -> bool,
    ) -> bool {
        let down = |action| i8::from(held(self.key(action)));
        snapshot.move_forward = down(Forward) - down(Back);
        snapshot.move_strafe = down(Right) - down(Left);
        snapshot.look_delta.0 *= self.sensitivity;
        snapshot.look_delta.1 *= self.sensitivity;
        for (keys, buttons) in [
            (&mut snapshot.keys_pressed, &mut snapshot.mouse_pressed),
            (&mut snapshot.keys_released, &mut snapshot.mouse_released),
        ] {
            let fire = match self.fire {
                FireBinding::Mouse(button) => buttons.contains(&button),
                FireBinding::Key(key) => keys.contains(&key),
            };
            buttons.clear();
            if fire {
                buttons.push(MouseButton::Left);
            }
            keys.retain_mut(|key| {
                if let Some(index) = self.keys.iter().position(|binding| binding == key) {
                    *key = KEYS[index].2;
                    true
                } else {
                    false
                }
            });
        }
        held(self.key(Sprint))
    }

    pub(crate) fn print(&self, actions: &[Action], fire: bool) {
        let mut labels = vec!["mouse look".to_string()];
        for action in [
            Forward, Back, Left, Right, Jump, Sprint, Camera, Overlay, Quit,
        ]
        .iter()
        .chain(actions)
        {
            labels.push(format!(
                "{} {}",
                self.key(*action),
                KEYS[*action as usize].1
            ));
        }
        if fire {
            let label = match self.fire {
                FireBinding::Mouse(button) => format!("{button:?} mouse"),
                FireBinding::Key(key) => key.name(),
            };
            labels.push(format!("{label} fire"));
        }
        println!("  controls: {}", labels.join(" · "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remapped_physical_controls_reach_the_gameplay_mapper() {
        use crate::encounter_demo::map_encounter_input;
        let controls =
            PlayerControls::parse(Some("forward=Up,jump=J,fire=F,camera=V"), Some("2")).unwrap();
        let mut snapshot = InputSnapshot {
            keys_pressed: vec![Keycode::J, Keycode::F, Keycode::V],
            look_delta: (2.0, -6.0),
            ..InputSnapshot::default()
        };
        let sprint = controls.map_frame(&mut snapshot, |key| {
            matches!(key, Keycode::Up | Keycode::LShift)
        });
        let mapped = map_encounter_input(&snapshot, sprint);
        assert!(
            mapped.intent.jump_pressed
                && mapped.fire_pressed
                && mapped.intent.sprint
                && mapped.toggle_camera
        );
        assert_eq!(mapped.intent.forward, 1);
        assert_eq!(
            mapped.intent.look,
            (
                -4.0 * crate::player_demo::DEMO_LOOK_SENSITIVITY,
                12.0 * crate::player_demo::DEMO_LOOK_SENSITIVITY
            )
        );
        let mut old_keys = InputSnapshot {
            keys_pressed: vec![Keycode::Space, Keycode::C],
            mouse_pressed: vec![MouseButton::Left],
            ..InputSnapshot::default()
        };
        controls.map_frame(&mut old_keys, |_| false);
        let mapped = map_encounter_input(&old_keys, false);
        assert!(!mapped.intent.jump_pressed && !mapped.fire_pressed && !mapped.toggle_camera);
        assert!(PlayerControls::parse(Some("jump=W"), None).is_err());
        assert!(PlayerControls::parse(Some("fire=unknown key"), None).is_err());
        assert!(PlayerControls::parse(None, Some("NaN")).is_err());
    }
}
