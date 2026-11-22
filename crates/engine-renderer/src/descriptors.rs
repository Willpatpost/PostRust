//! The descriptor-set foundation (the Cycle 6-A scaffold — see
//! `docs/CYCLE6-6A-BRIEF.md`). STUBBED: the contracts are the
//! architecture; the implementer (Astra/Sol per the owner's plan)
//! fills the Vulkan bodies.
//!
//! THE CONTRACTS (binding on the implementation):
//!
//! 1. NO PER-FRAME ALLOCATION: descriptor sets are allocated at
//!    init/recreate time, NEVER in the frame loop. The glyph atlas
//!    needs exactly ONE immutable set (bound every frame, written
//!    once): a combined image sampler over the atlas texture + the
//!    nearest sampler.
//! 2. BOUNDED, REPORTED GROWTH: the pool is created with an explicit
//!    maximum (the 6-A acceptance: one combined-image-sampler set;
//!    the pool sized for a documented headroom, the count reported —
//!    never silent growth, the audit's P1-4 discipline).
//! 3. LIFETIME: sets live for the renderer's lifetime; the pool is
//!    reset NEVER (no frees, no per-frame reset — static bindings);
//!    `destroy` releases the pool after the sets stop being used
//!    (the ordered teardown, images before device).
//! 4. LAYOUT: the 6-A layout is a single binding — binding 0,
//!    FRAGMENT shader, one combined image sampler. The scene
//!    pipeline's push-constant layout is UNTOUCHED (the atlas set is
//!    additive; the overlay pipeline gains the set, the scene
//!    pipeline does not).
//! 5. FAIL-CLOSED: pool creation or set allocation failure returns
//!    the typed error with partial-init cleanup (the
//!    PipelineState::new discipline: dead handles start null, destroy
//!    releases exactly what exists).

use ash::vk;

use crate::RendererError;

/// The 6-A descriptor infrastructure: one pool, one layout, one
/// immutable combined-image-sampler set. NOT YET WIRED — see the
/// module doc.
#[allow(dead_code)]
pub struct DescriptorFoundation {
    pub pool: vk::DescriptorPool,
    pub layout: vk::DescriptorSetLayout,
    pub set: vk::DescriptorSet,
}

impl DescriptorFoundation {
    /// Creates the pool, the single-binding layout, allocates the
    /// set, and writes the atlas binding into it.
    ///
    /// STUB — the implementer fills this in. `atlas_view` and
    /// `sampler` come from `textures::Texture`/`textures::SamplerPool`.
    #[allow(dead_code)]
    pub(crate) fn new(
        _ctx: &crate::vulkan::VulkanContext,
        _atlas_view: vk::ImageView,
        _sampler: vk::Sampler,
    ) -> Result<DescriptorFoundation, RendererError> {
        Err(RendererError::Vulkan(
            "DescriptorFoundation::new not implemented (the 6-A implementer's third body)".into(),
        ))
    }

    /// Idempotent teardown (pool last, after the sets are unused).
    #[allow(dead_code)]
    pub(crate) fn destroy(&mut self, _ctx: &crate::vulkan::VulkanContext) {
        // The implementer: destroy the layout, destroy the pool —
        // null-guarded, in that order.
    }
}
