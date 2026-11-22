//! The texture/image foundation (the Cycle 6-A scaffold — see
//! `docs/CYCLE6-6A-BRIEF.md`). STUBBED: the types and contracts are
//! the architecture; the implementer (Astra/Sol per the owner's
//! plan) fills the Vulkan bodies. NOT YET WIRED into the render path
//! — nothing here is called by production code yet (the
//! `#[allow(dead_code)]` allowances come off when the implementer
//! wires the integration).
//!
//! THE CONTRACTS (binding on the implementation):
//!
//! 1. DEVICE_LOCAL, ONE-TIME UPLOAD: textures are static assets —
//!    created once, uploaded once through a staging buffer (host
//!    visible -> transfer -> DEVICE_LOCAL image), never mapped again.
//!    The glyph atlas is the first customer (128x128 R8).
//! 2. BOUNDED ALLOCATION COUNT: each `Texture` performs at most ONE
//!    `vkAllocateMemory` (see the audit's P1-4: a block sub-allocator
//!    arrives when texture count grows; the 6-A acceptance allows the
//!    single dedicated allocation and REPORTS the count in the run
//!    summary — the discipline is a documented count, never silent
//!    growth).
//! 3. LIFETIME: `Texture::destroy` is idempotent (nulls its handles,
//!    frees exactly what exists) and is called from the owning
//!    renderer's ordered teardown — images before the device.
//! 4. LAYOUT: created directly in SHADER_READ_ONLY_OPTIMAL after the
//!    staging transfer (one barrier in the upload command buffer,
//!    never per frame).
//! 5. NO PER-FORMAT WORK: images use the exact format passed in; the
//!    implementer validates format support ONCE at creation
//!    (fail-closed with the typed error, never a fallback format).

use ash::vk;

use crate::RendererError;

/// A static, device-local, once-uploaded 2D image with its view.
/// NOT YET WIRED — see the module doc.
#[allow(dead_code)]
pub struct Texture {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
}

impl Texture {
    /// Creates the image, allocates memory, uploads `texels` through
    /// a one-shot staging command buffer, transitions to
    /// SHADER_READ_ONLY_OPTIMAL, and creates the view.
    ///
    /// STUB — the implementer fills this in. The stub exists so the
    /// module structure, contracts, and call sites compile; it
    /// returns the typed error until implemented.
    #[allow(dead_code)]
    pub(crate) fn upload(
        _ctx: &crate::vulkan::VulkanContext,
        _format: vk::Format,
        _extent: vk::Extent2D,
        _texels: &[u8],
    ) -> Result<Texture, RendererError> {
        Err(RendererError::Vulkan(
            "Texture::upload not implemented (the 6-A implementer's first body)".into(),
        ))
    }

    /// Idempotent teardown (the PipelineState::destroy discipline).
    ///
    /// STUB — fill alongside `upload`.
    #[allow(dead_code)]
    pub(crate) fn destroy(&mut self, _ctx: &crate::vulkan::VulkanContext) {
        // The implementer: destroy the view, free the memory, destroy
        // the image — each guarded by a null check, handles nulled
        // after release. Must be callable on a default/failed
        // texture (all-null) without effect.
    }
}

/// The sampler pool: immutable samplers created once, referenced by
/// handle. 6-A needs exactly one (nearest-filter for the glyph
/// atlas); the pool shape future-proofs linear/mipmap filtering
/// without per-frame sampler churn.
pub struct SamplerPool {
    samplers: Vec<vk::Sampler>,
}

impl SamplerPool {
    /// The nearest-filter sampler the glyph atlas requires (exact
    /// texel edges, no bleed between adjacent glyph cells).
    ///
    /// STUB — the implementer creates the pool with the single
    /// nearest sampler at renderer init and destroys it at teardown.
    #[allow(dead_code)]
    pub(crate) fn new(_ctx: &crate::vulkan::VulkanContext) -> Result<SamplerPool, RendererError> {
        Err(RendererError::Vulkan(
            "SamplerPool::new not implemented (the 6-A implementer's second body)".into(),
        ))
    }

    /// The nearest-filter sampler handle (index 0 in 6-A).
    #[allow(dead_code)]
    pub(crate) fn nearest(&self) -> vk::Sampler {
        self.samplers
            .first()
            .copied()
            .unwrap_or(vk::Sampler::null())
    }

    /// Idempotent teardown.
    #[allow(dead_code)]
    pub(crate) fn destroy(&mut self, _ctx: &crate::vulkan::VulkanContext) {
        // The implementer: destroy every sampler, clear the vec.
    }
}
