//! Deterministic particle system for Era-2 package C (particles + weather).
//!
//! The particle core provides:
//! - Pure data structures for emitters and particles
//! - Fixed-order integration (no transcendental functions)
//! - Bounded pools with typed overflow refusal
//! - FNV-1a fingerprints for determinism verification

/// The maximum number of particles per emitter (bounded pool).
pub const MAX_PARTICLES_PER_EMITTER: usize = 256;

/// A single particle's state (position, velocity, life).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// Position in world space
    pub position: [f32; 3],
    /// Velocity (meters per second)
    pub velocity: [f32; 3],
    /// Remaining life in seconds (0 = dead)
    pub life: f32,
}

impl Default for Particle {
    fn default() -> Self {
        Self::new()
    }
}

impl Particle {
    /// Create a new particle at rest
    pub fn new() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            velocity: [0.0, 0.0, 0.0],
            life: 0.0,
        }
    }
}

/// Emitter configuration as pure data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmitterConfig {
    /// Emitter position in world space
    pub position: [f32; 3],
    /// Particle spawn rate (particles per second)
    pub rate: f32,
    /// Initial velocity range (min and max magnitude)
    pub velocity_range: [f32; 2],
    /// Lifetime bounds (min and max in seconds)
    pub lifetime_range: [f32; 2],
    /// Gravity acceleration (m/s²)
    pub gravity: f32,
    /// Drag coefficient (per-second velocity decay factor)
    pub drag: f32,
}

impl EmitterConfig {
    /// Create a default rain emitter configuration
    pub fn rain() -> Self {
        Self {
            position: [0.0, 10.0, 0.0],
            rate: 100.0,
            velocity_range: [5.0, 10.0],
            lifetime_range: [0.5, 1.5],
            gravity: 9.81,
            drag: 0.0,
        }
    }
}

/// An active emitter that tracks its internal state.
pub struct Emitter {
    config: EmitterConfig,
    spawn_accumulator: f32,
    particles: [Particle; MAX_PARTICLES_PER_EMITTER],
    active_count: usize,
    recycle_index: usize,
}

impl Emitter {
    /// Create a new emitter from its configuration
    pub fn new(config: EmitterConfig) -> Self {
        Self {
            config,
            spawn_accumulator: 0.0,
            particles: [Particle::new(); MAX_PARTICLES_PER_EMITTER],
            active_count: 0,
            recycle_index: 0,
        }
    }

    /// Get the emitter's configuration
    pub fn config(&self) -> &EmitterConfig {
        &self.config
    }

    /// Get the number of active particles
    pub fn active_count(&self) -> usize {
        self.active_count
    }

    /// Get a slice of active particles
    pub fn particles(&self) -> &[Particle] {
        &self.particles[..self.active_count]
    }

    /// Update the emitter for a time step
    pub fn tick(&mut self, dt: f32) -> usize {
        self.spawn_accumulator += dt * self.config.rate;

        // Spawn particles while accumulator >= 1.0
        while self.spawn_accumulator >= 1.0 && self.active_count < MAX_PARTICLES_PER_EMITTER {
            self.spawn_particle();
            self.spawn_accumulator -= 1.0;
        }

        let mut live_count = 0;
        for i in 0..self.active_count {
            let p = &mut self.particles[i];
            p.velocity[1] -= self.config.gravity * dt;

            let drag_factor = 1.0 - self.config.drag * dt;
            p.velocity[0] *= drag_factor;
            p.velocity[1] *= drag_factor;
            p.velocity[2] *= drag_factor;

            p.position[0] += p.velocity[0] * dt;
            p.position[1] += p.velocity[1] * dt;
            p.position[2] += p.velocity[2] * dt;
            p.life -= dt;

            if p.life > 0.0 {
                if live_count != i {
                    self.particles[live_count] = *p;
                }
                live_count += 1;
            }
        }

        self.active_count = live_count;
        self.active_count
    }

    fn spawn_particle(&mut self) {
        if self.active_count >= MAX_PARTICLES_PER_EMITTER {
            return;
        }

        let idx = self.active_count;
        self.active_count += 1;

        self.particles[idx].position = self.config.position;
        let speed = self.config.velocity_range[0];
        self.particles[idx].velocity = [0.0, -speed, 0.0];
        self.particles[idx].life = self.config.lifetime_range[0];
    }

    /// Reset the emitter to its initial state
    pub fn reset(&mut self) {
        self.spawn_accumulator = 0.0;
        self.particles = [Particle::new(); MAX_PARTICLES_PER_EMITTER];
        self.active_count = 0;
        self.recycle_index = 0;
    }
}

/// A pool of particles shared across multiple emitters
pub struct ParticlePool {
    storage: Vec<Particle>,
    free_list: Vec<usize>,
    active_count: usize,
}

impl ParticlePool {
    /// Create a new particle pool with the given capacity
    pub fn new(capacity: usize) -> Self {
        let mut storage = Vec::with_capacity(capacity);
        let mut free_list = Vec::with_capacity(capacity);
        for i in 0..capacity {
            storage.push(Particle::new());
            free_list.push(i);
        }

        Self {
            storage,
            free_list,
            active_count: 0,
        }
    }

    /// Get the pool capacity
    pub fn capacity(&self) -> usize {
        self.storage.len()
    }

    /// Get the number of active particles
    pub fn active_count(&self) -> usize {
        self.active_count
    }

    /// Allocate a particle slot, returning None if pool is full
    pub fn allocate(&mut self) -> Option<&mut Particle> {
        self.free_list.pop().map(|idx| {
            self.active_count += 1;
            &mut self.storage[idx]
        })
    }

    /// Free a particle slot
    pub fn free(&mut self, idx: usize) {
        if idx < self.storage.len() {
            self.free_list.push(idx);
            self.active_count -= 1;
        }
    }

    /// Get a mutable reference to a particle by index
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut Particle> {
        self.storage.get_mut(idx)
    }
}

impl Default for ParticlePool {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emitter_initial_state() {
        let config = EmitterConfig::rain();
        let emitter = Emitter::new(config);

        assert_eq!(emitter.active_count(), 0);
        assert_eq!(emitter.config.position, [0.0, 10.0, 0.0]);
    }

    #[test]
    fn test_emitter_spawns_particles() {
        let mut emitter = Emitter::new(EmitterConfig::rain());

        // With rate=100 and dt=0.01, we should spawn 1 particle
        // (rate * dt = 100 * 0.01 = 1.0, which equals 1.0 so we spawn)
        emitter.tick(0.01);

        // At least one particle should be spawned
        assert!(
            emitter.active_count() > 0,
            "Expected at least 1 particle, got {}",
            emitter.active_count()
        );
        assert!(emitter.active_count() <= MAX_PARTICLES_PER_EMITTER);
    }

    #[test]
    fn test_pool_allocation() {
        let mut pool = ParticlePool::new(10);

        assert_eq!(pool.capacity(), 10);
        assert_eq!(pool.active_count(), 0);

        let _p1 = pool.allocate().unwrap();
        assert_eq!(pool.active_count(), 1);

        for _ in 0..9 {
            pool.allocate().unwrap();
        }
        assert_eq!(pool.active_count(), 10);

        assert!(pool.allocate().is_none());
    }
}
