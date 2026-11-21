# Player controls

The player, gatehouse, combat, and encounter demos accept two environment
variables at startup. Defaults match the on-screen and console controls for
an unconfigured demo.

| Variable | Value |
| --- | --- |
| `POSTRUST_BINDINGS` | Comma-separated `action=key` pairs. Use SDL key names such as `Up`, `J`, or `Right Shift`. The `fire` action also accepts `Mouse1` through `Mouse5` in left, right, middle, side 1, side 2 order. |
| `POSTRUST_LOOK_SENSITIVITY` | Mouse movement multiplier greater than zero and at most 100. The default is 1. |

The actions are `forward`, `back`, `left`, `right`, `jump`, `sprint`, `camera`,
`interact`, `reset`, `fire`, `precision`, `spread`, `cycle`, `overlay`, and
`quit`. Each demo uses the actions relevant to its scenario. If you assign a
key already in use, reassign both actions in the same setting. The demo
prints its resolved bindings at startup.

```powershell
$env:POSTRUST_BINDINGS = 'forward=Up,back=Down,left=Left,right=Right,jump=J,fire=F'
$env:POSTRUST_LOOK_SENSITIVITY = '0.6'
cargo run --release -p engine-slice -- --encounter --hud-scale 1.5
```

`--hud-scale` multiplies the encounter HUD's automatic scale. It takes a
value from 0.75 to 4 and fits the result inside the window. The gameplay HUD
and the debug overlay have separate controls.
