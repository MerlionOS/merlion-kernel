/// Simple 2-D physics engine for MerlionOS games and demos.
/// Provides circle-based rigid-body dynamics with Euler integration,
/// gravity, elastic collisions, and rectangular boundary walls.
/// Designed for `#![no_std]` kernel use with the `alloc` crate.

use alloc::vec::Vec;

/// A two-dimensional vector with integer components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: i32,
    /// Vertical component.
    pub y: i32,
}

impl Vec2 {
    /// Creates a new vector.
    pub const fn new(x: i32, y: i32) -> Self { Self { x, y } }
    /// The zero vector.
    pub const ZERO: Vec2 = Vec2 { x: 0, y: 0 };

    /// Component-wise addition.
    pub fn add(self, o: Vec2) -> Vec2 { Vec2 { x: self.x + o.x, y: self.y + o.y } }
    /// Component-wise subtraction.
    pub fn sub(self, o: Vec2) -> Vec2 { Vec2 { x: self.x - o.x, y: self.y - o.y } }
    /// Scales both components by a scalar.
    pub fn scale(self, s: i32) -> Vec2 { Vec2 { x: self.x * s, y: self.y * s } }
    /// Divides both components by a divisor (integer division).
    pub fn div(self, d: i32) -> Vec2 { Vec2 { x: self.x / d, y: self.y / d } }
    /// Dot product of two vectors.
    pub fn dot(self, o: Vec2) -> i64 { (self.x as i64) * (o.x as i64) + (self.y as i64) * (o.y as i64) }
    /// Squared magnitude (avoids square root).
    pub fn magnitude_sq(self) -> i64 { self.dot(self) }
    /// Approximate magnitude using integer square root.
    pub fn magnitude(self) -> i32 { isqrt(self.magnitude_sq()) }
}

/// Integer square root via Newton's method.
fn isqrt(n: i64) -> i32 {
    if n <= 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x { x = y; y = (x + n / x) / 2; }
    x as i32
}

/// A rigid body represented as a circle with position, velocity,
/// acceleration, mass, and radius.
#[derive(Debug, Clone)]
pub struct Body {
    /// Position in world coordinates.
    pub pos: Vec2,
    /// Velocity (units per tick).
    pub vel: Vec2,
    /// Accumulated acceleration applied next step.
    pub acc: Vec2,
    /// Mass (arbitrary units, must be > 0).
    pub mass: i32,
    /// Collision radius.
    pub radius: i32,
    /// If `true` the body is immovable (e.g. a wall anchor).
    pub fixed: bool,
}

impl Body {
    /// Creates a new body at the given position with zero velocity.
    pub fn new(x: i32, y: i32, mass: i32, radius: i32) -> Self {
        Self { pos: Vec2::new(x, y), vel: Vec2::ZERO, acc: Vec2::ZERO, mass, radius, fixed: false }
    }

    /// Creates a fixed (immovable) body.
    pub fn new_fixed(x: i32, y: i32, radius: i32) -> Self {
        Self { pos: Vec2::new(x, y), vel: Vec2::ZERO, acc: Vec2::ZERO, mass: i32::MAX, radius, fixed: true }
    }

    /// Returns the kinetic energy (integer approximation).
    pub fn kinetic_energy(&self) -> i64 { (self.mass as i64) * self.vel.magnitude_sq() / 2 }
}

/// Maximum bodies the world can track.
const MAX_BODIES: usize = 256;

/// A 2-D physics world holding bodies and providing integration,
/// collision detection/resolution, and boundary enforcement.
pub struct World {
    /// All bodies in the simulation.
    pub bodies: Vec<Body>,
    /// Gravity vector applied every step.
    pub gravity: Vec2,
    /// Simulation tick counter.
    pub tick: u64,
}

impl World {
    /// Creates an empty world with no gravity.
    pub fn new() -> Self {
        Self { bodies: Vec::new(), gravity: Vec2::ZERO, tick: 0 }
    }

    /// Adds a body to the world. Returns its index, or `None` if full.
    pub fn add_body(&mut self, body: Body) -> Option<usize> {
        if self.bodies.len() >= MAX_BODIES { return None; }
        let id = self.bodies.len();
        self.bodies.push(body);
        Some(id)
    }

    /// Removes a body by index (swap-remove). Returns `true` on success.
    pub fn remove_body(&mut self, id: usize) -> bool {
        if id < self.bodies.len() { self.bodies.swap_remove(id); true } else { false }
    }

    /// Sets the global gravity vector.
    pub fn apply_gravity(&mut self, g: Vec2) { self.gravity = g; }

    /// Applies an external force to a specific body (F/m added to acceleration).
    pub fn apply_force(&mut self, body_id: usize, force: Vec2) {
        if let Some(body) = self.bodies.get_mut(body_id) {
            if !body.fixed {
                body.acc = body.acc.add(force.div(body.mass));
            }
        }
    }

    /// Advances the simulation by `dt` ticks using semi-implicit Euler
    /// integration.
    pub fn step(&mut self, dt: i32) {
        let g = self.gravity;
        // Integration pass.
        for body in self.bodies.iter_mut() {
            if body.fixed { continue; }
            let total_acc = body.acc.add(g);
            body.vel = body.vel.add(total_acc.scale(dt));  // v += a * dt
            body.pos = body.pos.add(body.vel.scale(dt));   // p += v * dt
            body.acc = Vec2::ZERO;
        }
        // Collision detection & resolution (O(n^2) brute force).
        let n = self.bodies.len();
        for i in 0..n {
            for j in (i + 1)..n {
                if Self::check_collision_pair(&self.bodies[i], &self.bodies[j]) {
                    let ptr = self.bodies.as_mut_ptr();
                    unsafe { Self::resolve_collision(&mut *ptr.add(i), &mut *ptr.add(j)); }
                }
            }
        }
        self.tick += 1;
    }

    /// Returns `true` when the circles of bodies `a` and `b` overlap.
    pub fn check_collision(a: &Body, b: &Body) -> bool {
        Self::check_collision_pair(a, b)
    }

    /// Internal collision test using squared distances.
    fn check_collision_pair(a: &Body, b: &Body) -> bool {
        let dx = (a.pos.x - b.pos.x) as i64;
        let dy = (a.pos.y - b.pos.y) as i64;
        let dist_sq = dx * dx + dy * dy;
        let min_dist = (a.radius + b.radius) as i64;
        dist_sq <= min_dist * min_dist
    }

    /// Resolves an elastic collision between two circles by adjusting
    /// velocities and separating overlapping bodies.
    fn resolve_collision(a: &mut Body, b: &mut Body) {
        if a.fixed && b.fixed { return; }
        let delta = a.pos.sub(b.pos);
        let dist = delta.magnitude();
        if dist == 0 { a.pos.x += 1; return; } // nudge apart

        // Separate overlapping bodies.
        let overlap = a.radius + b.radius - dist;
        if overlap > 0 {
            let half = overlap / 2 + 1;
            let nx = delta.x * half / dist;
            let ny = delta.y * half / dist;
            if !a.fixed { a.pos.x += nx; a.pos.y += ny; }
            if !b.fixed { b.pos.x -= nx; b.pos.y -= ny; }
        }

        // Elastic velocity exchange along collision normal.
        let rel = a.vel.sub(b.vel);
        let normal = delta;
        let dot = rel.dot(normal);
        if dot >= 0 { return; } // already separating

        let dist_sq = (dist as i64) * (dist as i64);
        let total_mass = if a.fixed || b.fixed { 1 } else { a.mass + b.mass };

        if a.fixed {
            let s = 2 * dot / dist_sq;
            b.vel.x += (s * normal.x as i64) as i32;
            b.vel.y += (s * normal.y as i64) as i32;
        } else if b.fixed {
            let s = 2 * dot / dist_sq;
            a.vel.x -= (s * normal.x as i64) as i32;
            a.vel.y -= (s * normal.y as i64) as i32;
        } else {
            let sa = 2 * (b.mass as i64) * dot / ((total_mass as i64) * dist_sq);
            let sb = 2 * (a.mass as i64) * dot / ((total_mass as i64) * dist_sq);
            a.vel.x -= (sa * normal.x as i64) as i32;
            a.vel.y -= (sa * normal.y as i64) as i32;
            b.vel.x += (sb * normal.x as i64) as i32;
            b.vel.y += (sb * normal.y as i64) as i32;
        }
    }

    /// Keeps every body inside a `width x height` rectangle starting at
    /// the origin. Bodies that hit a wall have their velocity reflected.
    pub fn bounds_check(&mut self, width: i32, height: i32) {
        for body in self.bodies.iter_mut() {
            if body.fixed { continue; }
            if body.pos.x - body.radius < 0 {
                body.pos.x = body.radius;
                body.vel.x = -body.vel.x;
            }
            if body.pos.x + body.radius > width {
                body.pos.x = width - body.radius;
                body.vel.x = -body.vel.x;
            }
            if body.pos.y - body.radius < 0 {
                body.pos.y = body.radius;
                body.vel.y = -body.vel.y;
            }
            if body.pos.y + body.radius > height {
                body.pos.y = height - body.radius;
                body.vel.y = -body.vel.y;
            }
        }
    }

    /// Returns the number of bodies currently in the world.
    pub fn body_count(&self) -> usize { self.bodies.len() }
}

/// Sets up a demo world with five bouncing balls inside an 800x600
/// arena and runs 500 simulation steps, returning final positions.
pub fn bouncing_balls_demo() -> Vec<(i32, i32)> {
    let mut world = World::new();
    world.apply_gravity(Vec2::new(0, 2)); // mild downward gravity

    // Ball 1 -- top-left, moving right and down.
    let mut b1 = Body::new(100, 100, 10, 15);
    b1.vel = Vec2::new(5, 3);
    world.add_body(b1);

    // Ball 2 -- centre, moving left.
    let mut b2 = Body::new(400, 300, 20, 20);
    b2.vel = Vec2::new(-4, 1);
    world.add_body(b2);

    // Ball 3 -- bottom-right, moving up-left.
    let mut b3 = Body::new(700, 500, 15, 18);
    b3.vel = Vec2::new(-3, -6);
    world.add_body(b3);

    // Ball 4 -- small fast ball.
    let mut b4 = Body::new(200, 450, 5, 10);
    b4.vel = Vec2::new(7, -2);
    world.add_body(b4);

    // Ball 5 -- heavy slow ball.
    let mut b5 = Body::new(600, 150, 40, 25);
    b5.vel = Vec2::new(-1, 4);
    world.add_body(b5);

    // Run 500 steps inside 800x600 bounds.
    for _ in 0..500 {
        world.step(1);
        world.bounds_check(800, 600);
    }

    world.bodies.iter().map(|b| (b.pos.x, b.pos.y)).collect()
}
