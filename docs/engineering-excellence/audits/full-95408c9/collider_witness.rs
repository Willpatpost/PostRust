use engine_core::RotatedCollider;

fn main() {
    let mut moved = RotatedCollider::new([0.0; 3], [1.0; 3], 0.0).unwrap();
    moved.center[0] = 10.0;
    let rebuilt = RotatedCollider::new(moved.center, moved.half, moved.yaw).unwrap();
    let min = [9.5, -0.5, -0.5];
    let max = [10.5, 0.5, 0.5];
    println!(
        "moved={:?} rebuilt={:?}",
        moved.world_aabb(),
        rebuilt.world_aabb()
    );
    assert_eq!(moved.overlaps_box(min, max), rebuilt.overlaps_box(min, max));
}
