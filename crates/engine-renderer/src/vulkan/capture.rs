//! Capture and diagnostics (P4 S4 — design §5.1's capture row): the
//! bounded frame-capture readback (types, destination state, and the
//! single-flight request/take handshake), the shadow-map readback
//! handshake, and the validation-diagnostics queue with its drain (the
//! run summary's environment channel). The production frame path
//! records the readback passes only when the frame state's handshake
//! says so. Pure move from vulkan.rs (stage S4); no logic changed —
//! only location and visibility.

use std::ffi::CStr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use super::RendererError;
use super::WAIT_TIMEOUT_NS;
use super::context::VulkanContext;
use super::frame::FrameState;
use super::pipelines::PipelineState;
use super::resources::create_mapped_buffer;
use super::swapchain::SwapchainState;

/// Validation messages from the driver callback: (class, text) where
/// class 2 = error, 1 = warning, 0 = info. Bounded; drained by
/// [`crate::Renderer`] each frame.
static VALIDATION_QUEUE: Mutex<Vec<(u8, String)>> = Mutex::new(Vec::new());

/// Updates to the hint are serialized with queue mutations. An inactive
/// renderer can skip the queue mutex when no environment message is pending.
static ENVIRONMENT_PENDING: AtomicBool = AtomicBool::new(false);

/// Whether the queue holds environment diagnostics awaiting a drain
/// (cheap atomic read — the per-frame drain gate consults this).
pub(crate) fn environment_pending() -> bool {
    ENVIRONMENT_PENDING.load(Ordering::Relaxed)
}

/// Takes the queued messages and clears their hint under the same lock.
pub(crate) fn drain_validation_messages() -> Vec<(u8, String)> {
    let mut queue = VALIDATION_QUEUE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    ENVIRONMENT_PENDING.store(false, Ordering::Relaxed);
    std::mem::take(&mut *queue)
}

/// Records a renderer-observed environment limitation into the
/// validation diagnostics queue (class 1 = warning) — the existing
/// channel every run summary drains, so the limitation is explicitly
/// diagnosable without new architecture (Mac 2 review follow-up B).
pub(super) fn queue_environment_message(text: String) {
    if let Ok(mut queue) = VALIDATION_QUEUE.lock()
        && queue.len() < 256
    {
        queue.push((1, text));
        ENVIRONMENT_PENDING.store(true, Ordering::Relaxed);
    }
}

pub(super) unsafe extern "system" fn validation_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT,
    _user_data: *mut core::ffi::c_void,
) -> vk::Bool32 {
    if let Some(record) = (!data.is_null()).then(|| unsafe { &*data }) {
        let text = unsafe { CStr::from_ptr(record.p_message) }
            .to_string_lossy()
            .into_owned();
        let class = if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
            2
        } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
            1
        } else {
            0
        };
        if let Ok(mut queue) = VALIDATION_QUEUE.lock()
            && queue.len() < 256
        {
            queue.push((class, text));
        }
    }
    vk::FALSE
}

/// The capture API's supported swapchain formats (the
/// post-orientation hardening's format contract): the 4-byte-per-
/// pixel RGBA8/BGRA8 family — UNORM and sRGB variants share the byte
/// footprint and channel order semantics. Returns the per-pixel byte
/// footprint, or `None` for every format outside the family: a
/// fallback surface format such as `R16G16B16A16_SFLOAT` (8 bytes per
/// pixel) must be REFUSED before any copy command is recorded, never
/// under-allocated and overwritten. The ordinary renderer's
/// surface-format fallback behavior is untouched — only capture
/// refuses.
pub(crate) fn capture_format_footprint(format: vk::Format) -> Option<usize> {
    match format {
        vk::Format::B8G8R8A8_UNORM
        | vk::Format::B8G8R8A8_SRGB
        | vk::Format::R8G8B8A8_UNORM
        | vk::Format::R8G8B8A8_SRGB => Some(4),
        _ => None,
    }
}

/// The capture destination's byte length, via checked arithmetic —
/// `None` when the width × height × footprint product overflows
/// usize (the typed [`RendererError::CaptureSizingOverflow`] refusal).
pub(crate) fn capture_byte_len(width: u32, height: u32, footprint: usize) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(footprint))
}

/// One captured frame's readback (the bounded capture path — the
/// orientation corrective's visual-evidence route).
///
/// The bytes are the swapchain image's texels as presented: tightly
/// packed rows, TOP row first (the framebuffer origin), one pixel per
/// 4 bytes in the swapchain's channel order (`format`). The capture
/// is taken AFTER the frame's render pass and BEFORE presentation —
/// exactly the image the display receives.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Tightly packed RGBA8-family texel rows, top row first.
    pub bytes: Vec<u8>,
    /// The swapchain format the bytes were copied in.
    pub format: CaptureFormat,
}

/// The capture's channel order (the swapchain's picked format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    /// Byte order B, G, R, A — the renderer's preferred
    /// `B8G8R8A8_UNORM` surface.
    Bgra8,
    /// Byte order R, G, B, A.
    Rgba8,
    /// Any other format: the raw bytes are returned, but
    /// [`CapturedFrame::pixel_rgba`] refuses to interpret them.
    Other,
}

impl CapturedFrame {
    /// The RGBA tuple at pixel `(x, y)` — row 0 is the TOP row, x from
    /// the left (the framebuffer convention; the projection-orientation
    /// regressions assert against this). Panics for `Other` formats.
    pub fn pixel_rgba(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "pixel ({x}, {y}) outside the {width}×{height} capture",
            width = self.width,
            height = self.height
        );
        let index = (y * self.width + x) as usize * 4;
        match self.format {
            CaptureFormat::Bgra8 => [
                self.bytes[index + 2],
                self.bytes[index + 1],
                self.bytes[index],
                self.bytes[index + 3],
            ],
            CaptureFormat::Rgba8 => [
                self.bytes[index],
                self.bytes[index + 1],
                self.bytes[index + 2],
                self.bytes[index + 3],
            ],
            CaptureFormat::Other => {
                panic!(
                    "the captured format is not an RGBA8-family format — refusing to guess the channel order"
                )
            }
        }
    }
}

/// The lazily-allocated frame-capture readback state (the orientation
/// corrective): a persistently mapped host-visible destination buffer
/// plus the one-shot request handshake. Allocated on first request and
/// reallocated when the swapchain extent changes; a capture records a
/// layout barrier pair and a CopyImageToBuffer into the frame's own
/// command buffer, so the slot's submission fence proves completion.
pub(crate) struct CaptureState {
    pub(super) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut core::ffi::c_void,
    pub(super) byte_len: usize,
    width: u32,
    height: u32,
    format: vk::Format,
    /// A capture was requested for the next recorded frame (survives
    /// deferred frames — retried until a frame actually records).
    pub(super) pending: bool,
    /// A capture was recorded; `take_capture` waits the slot's fence.
    pub(super) ready: bool,
    pub(super) slot: usize,
    /// The most recent refusal or drop (None in the ordinary path) —
    /// set when a pending request is refused or dropped so the frame
    /// can render normally; observable through
    /// [`Renderer::last_capture_refusal`].
    pub(super) last_refusal: Option<RendererError>,
}

impl CaptureState {
    pub(super) fn dead() -> Self {
        Self {
            buffer: vk::Buffer::null(),
            memory: vk::DeviceMemory::null(),
            mapped: core::ptr::null_mut(),
            byte_len: 0,
            width: 0,
            height: 0,
            format: vk::Format::UNDEFINED,
            pending: false,
            ready: false,
            slot: 0,
            last_refusal: None,
        }
    }

    pub(super) fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            if self.buffer != vk::Buffer::null() {
                if !self.mapped.is_null() {
                    ctx.device.unmap_memory(self.memory);
                }
                ctx.device.destroy_buffer(self.buffer, None);
                self.buffer = vk::Buffer::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                ctx.device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
            self.mapped = core::ptr::null_mut();
            self.ready = false;
        }
    }

    /// (Re)allocates for the current swapchain extent, OUTSIDE command
    /// recording (the same discipline as the lazy region buffer). A
    /// no-op when the existing buffer already matches. The FORMAT
    /// CONTRACT and the checked sizing run FIRST — pure refusals that
    /// touch no GPU state, so an unsupported surface format (e.g. an
    /// 8-byte-per-pixel fallback) is refused with a typed error before
    /// any allocation or copy command, and the byte length can never
    /// exceed what the format actually requires. The TRANSFER-SOURCE
    /// capability is validated INDEPENDENTLY of destination
    /// reusability (the cache-validation corrective's defect 2): a
    /// recreated swapchain without TRANSFER_SRC is refused even when
    /// the cached destination would otherwise be reused — and the
    /// refusal stays pure (no GPU state is touched). The cache reuse
    /// itself validates the FULL extent (the corrective's defect 1):
    /// width AND height, never just the byte count — 1280×720 and
    /// 960×960 share a pixel count but not a shape, and the reported
    /// capture dimensions must always correspond to the image
    /// actually copied.
    pub(super) fn ensure(
        &mut self,
        ctx: &VulkanContext,
        swapchain: &SwapchainState,
    ) -> Result<(), RendererError> {
        let footprint = capture_format_footprint(swapchain.format).ok_or_else(|| {
            RendererError::CaptureUnsupportedFormat(format!("{:?}", swapchain.format))
        })?;
        let width = swapchain.extent.width;
        let height = swapchain.extent.height;
        let needed = capture_byte_len(width, height, footprint)
            .ok_or(RendererError::CaptureSizingOverflow)?;
        if !swapchain.transfer_src_supported {
            return Err(RendererError::NoDevice(
                "the surface's swapchain images do not support TRANSFER_SRC — frame \
                 capture unavailable"
                    .into(),
            ));
        }
        if self.buffer != vk::Buffer::null()
            && self.width == width
            && self.height == height
            && self.format == swapchain.format
        {
            return Ok(());
        }
        self.destroy(ctx);
        let (buffer, memory, mapped) = create_mapped_buffer(
            ctx,
            vk::BufferUsageFlags::TRANSFER_DST,
            needed as vk::DeviceSize,
        )?;
        self.buffer = buffer;
        self.memory = memory;
        self.mapped = mapped;
        self.byte_len = needed;
        self.width = width;
        self.height = height;
        self.format = swapchain.format;
        Ok(())
    }

    /// The record-time guard predicate (the cache-validation
    /// corrective): the prepared destination may serve the image
    /// being recorded ONLY when the FULL extent (width AND height —
    /// never just the byte count), the format, AND the current
    /// swapchain's TRANSFER-SOURCE capability all match — a
    /// mid-frame swapchain recreation may change the shape, the
    /// format, or the capability between the pre-acquire preparation
    /// and the recording.
    pub(super) fn matches(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
        transfer_src: bool,
    ) -> bool {
        self.buffer != vk::Buffer::null()
            && transfer_src
            && self.width == width
            && self.height == height
            && self.format == format
    }
}

/// Requests that the NEXT recorded frame be copied back to the host.
/// ONE capture outstanding at a time (the bounded single-flight
/// lifecycle policy): refused with a typed error while an earlier
/// request is pending (not yet recorded) or ready-but-unconsumed — a
/// second copy could otherwise overwrite the earlier readback's
/// destination while it is still unconsumed.
pub(crate) fn request_capture(frames: &mut FrameState) -> Result<(), RendererError> {
    if frames.capture.pending || frames.capture.ready {
        return Err(RendererError::CaptureOutstanding);
    }
    frames.capture.last_refusal = None;
    frames.capture.pending = true;
    Ok(())
}

/// Requests the NEXT shadowed frame's shadow-map readback (the 6-F
/// known-answer path): the depth-only pass's map is copied to a host
/// buffer inside that frame's command buffer. Single-flight like the
/// color capture. The handshake lives in the frame state, so the
/// request survives pipeline replacements; the shadow STATE itself
/// must exist (the renderer's request path ensures it). A plain frame
/// (no `shadowed` params) does NOT consume the request — it survives
/// until the next SHADOWED frame runs the depth pass.
pub(crate) fn request_shadow_readback(frames: &mut FrameState) -> Result<(), RendererError> {
    if frames.shadow_readback.pending || frames.shadow_readback.ready {
        return Err(RendererError::CaptureOutstanding);
    }
    frames.shadow_readback.pending = true;
    Ok(())
}

/// Takes the completed shadow-map readback, if one is ready: waits
/// the recording slot's fence (the color capture's take discipline)
/// and returns the map's depths (row-major, row 0 first, one f32 per
/// texel — the D32_SFLOAT bits as written by the depth-only pass).
/// Propagates fence-wait errors; on error `ready` stays true so the
/// caller may retry (exactly `take_capture`'s discipline). Returns
/// `CaptureDropped` if the shadow state (or its buffer) was replaced
/// after the readback recorded — the recording buffer was destroyed
/// with it.
pub(crate) fn take_shadow_readback(
    ctx: &VulkanContext,
    pipes: &mut PipelineState,
    frames: &mut FrameState,
) -> Result<Option<Vec<f32>>, RendererError> {
    if !frames.shadow_readback.ready {
        return Ok(None);
    }
    // If the shadow state was replaced after the readback recorded,
    // the buffer handle changed — the old buffer is gone, and reading
    // the new one's fresh allocation would return undefined data.
    let shadows = match pipes.shadows.as_ref() {
        Some(shadows) => {
            if shadows.readback_buffer() != frames.shadow_readback.buffer {
                frames.shadow_readback.ready = false;
                return Err(RendererError::CaptureDropped(
                    "the shadow state was replaced after the readback recorded — the recording buffer was destroyed with it".into(),
                ));
            }
            shadows
        }
        None => {
            // The state was replaced after the readback recorded: the
            // buffer holding the copy is gone — a typed drop (the color
            // capture's mismatch path), reported rather than silently
            // empty.
            frames.shadow_readback.ready = false;
            return Err(RendererError::CaptureDropped(
                "the shadow state was replaced before the readback was taken".into(),
            ));
        }
    };
    let slot = frames.shadow_readback.slot;
    // SAFETY: the fence is the recording frame's own — the same
    // fence the next frame on that slot waits.
    let depths = unsafe { shadows.readback_depths(ctx, frames.in_flight[slot]) }?;
    // Only clear ready after a successful copy; on error the caller
    // may retry.
    frames.shadow_readback.ready = false;
    Ok(Some(depths))
}

/// The most recent capture refusal or drop, if any — observable for
/// diagnosis; a refused capture never poisons frame synchronization.
pub(crate) fn last_capture_refusal(frames: &FrameState) -> Option<&RendererError> {
    frames.capture.last_refusal.as_ref()
}

/// Takes the completed capture: waits the recording slot's fence (the
/// same fence the next frame waits — a later submission on that slot
/// can only have started after this capture's fence signaled), copies
/// the mapped bytes out, and clears the handshake. `Ok(None)` when no
/// capture is ready.
pub(crate) fn take_capture(
    ctx: &VulkanContext,
    frames: &mut FrameState,
) -> Result<Option<CapturedFrame>, RendererError> {
    if !frames.capture.ready {
        return Ok(None);
    }
    let slot = frames.capture.slot;
    let (byte_len, width, height, captured_format) = {
        let capture = &frames.capture;
        (
            capture.byte_len,
            capture.width,
            capture.height,
            capture.format,
        )
    };
    unsafe {
        ctx.device
            .wait_for_fences(&[frames.in_flight[slot]], true, WAIT_TIMEOUT_NS)
            .map_err(|error| RendererError::vk(error, "wait_for_fences (capture)"))?;
    }
    let mut bytes = vec![0u8; byte_len];
    unsafe {
        std::ptr::copy_nonoverlapping(
            frames.capture.mapped as *const u8,
            bytes.as_mut_ptr(),
            byte_len,
        );
    }
    frames.capture.ready = false;
    let format = match captured_format {
        vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB => CaptureFormat::Bgra8,
        vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB => CaptureFormat::Rgba8,
        _ => CaptureFormat::Other,
    };
    Ok(Some(CapturedFrame {
        width,
        height,
        bytes,
        format,
    }))
}

/// The shadow-map readback's single-flight handshake (the color
/// capture's shape; lives in `FrameState` so a request survives
/// pipeline replacements — the shadow STATE is rebuilt, the request
/// is not). The buffer field pins the producing generation: the
/// readback records with that buffer's handle; `take` returns
/// `CaptureDropped` if the buffer was destroyed (a replacement
/// installed after the recording but before the take).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShadowReadbackState {
    pub(super) pending: bool,
    pub(super) ready: bool,
    pub(super) slot: usize,
    pub(super) buffer: vk::Buffer,
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn environment_enumeration_failure_is_queued_as_a_warning() {
        // Mac 2 review follow-up B: a loader-enumeration failure must
        // be explicitly diagnosable — it rides the existing validation
        // diagnostics queue the run summary drains.
        drain_validation_messages();
        queue_environment_message("enumeration probe".to_string());
        assert!(environment_pending());
        let queued = drain_validation_messages();
        assert!(!environment_pending());
        assert_eq!(queued, vec![(1, String::from("enumeration probe"))]);
    }

    /// The post-orientation hardening's H3 format contract: the
    /// supported set is EXACTLY the 4-byte-per-pixel RGBA8/BGRA8
    /// family; everything else — including the audited 8-byte
    /// fallbacks that would overflow a `width × height × 4`
    /// destination — is refused (typed, before any copy command).
    #[test]
    fn capture_formats_are_the_four_byte_family_only() {
        use super::capture_format_footprint;
        use ash::vk;

        for format in [
            vk::Format::B8G8R8A8_UNORM,
            vk::Format::B8G8R8A8_SRGB,
            vk::Format::R8G8B8A8_UNORM,
            vk::Format::R8G8B8A8_SRGB,
        ] {
            assert_eq!(
                capture_format_footprint(format),
                Some(4),
                "the supported family: {format:?}"
            );
        }
        for format in [
            vk::Format::R16G16B16A16_SFLOAT,
            vk::Format::R16G16B16A16_UNORM,
            vk::Format::R32G32B32A32_SFLOAT,
            vk::Format::B8G8R8A8_SNORM,
            vk::Format::R5G6B5_UNORM_PACK16,
            vk::Format::A8B8G8R8_UNORM_PACK32,
        ] {
            assert_eq!(
                capture_format_footprint(format),
                None,
                "outside the family — refused, never under-allocated: {format:?}"
            );
        }
    }

    /// The post-orientation hardening's H3 checked sizing: real
    /// extents fit; the u32-extreme product overflows usize and
    /// refuses rather than wrapping.
    #[test]
    fn capture_sizing_is_checked() {
        use super::capture_byte_len;
        assert_eq!(capture_byte_len(1920, 1080, 4), Some(1920 * 1080 * 4));
        assert_eq!(
            capture_byte_len(u32::MAX, u32::MAX, 4),
            None,
            "u32-extreme extents must refuse instead of wrapping"
        );
    }

    /// The final cache-validation corrective's defect-1 regression: a
    /// cached 1280×720 destination must NOT be reused for a 960×960
    /// swapchain even though the two extents share EXACTLY the same
    /// pixel count (and therefore byte length) — the reported capture
    /// dimensions must always correspond to the image actually
    /// copied. Fail-first at the held candidate: the reuse predicate
    /// compared only the byte length and the format, so this
    /// FAILED before the fix (log cc-failfirst-defect1.log).
    #[test]
    fn capture_cache_reuse_validates_the_full_extent() {
        use super::{CaptureState, capture_byte_len};
        use ash::vk;
        use ash::vk::Handle;

        let cached = CaptureState {
            buffer: vk::Buffer::from_raw(1),
            byte_len: capture_byte_len(1280, 720, 4).expect("the cached size"),
            width: 1280,
            height: 720,
            format: vk::Format::B8G8R8A8_UNORM,
            ..CaptureState::dead()
        };
        // The defect's exact shape: same pixel count, different
        // dimensions — must be rejected.
        assert!(
            !cached.matches(960, 960, vk::Format::B8G8R8A8_UNORM, true),
            "a 1280×720 cache must not serve a 960×960 swapchain (same \
             byte count, different shape)"
        );
        // The matching extent still reuses (the positive control —
        // the hot path does not reallocate).
        assert!(cached.matches(1280, 720, vk::Format::B8G8R8A8_UNORM, true));
        // A format change is likewise rejected even at the same
        // extent.
        assert!(!cached.matches(1280, 720, vk::Format::B8G8R8A8_SRGB, true));
    }

    /// The final cache-validation corrective's defect-2 regression:
    /// the record-time guard requires the CURRENT swapchain's
    /// transfer-source capability even when the destination is
    /// otherwise a perfect match — a recreated swapchain without
    /// TRANSFER_SRC must never reach the image-to-buffer copy. The
    /// same predicate drives `ensure()`'s reuse decision, with the
    /// capability checked FIRST and independently (the refusal stays
    /// pure). Fail-first: with the capability term disabled (the
    /// pre-fix defect form), this test fails (log
    /// cc-failfirst-defect2.log).
    #[test]
    fn capture_record_guard_requires_transfer_source() {
        use super::CaptureState;
        use ash::vk;
        use ash::vk::Handle;

        let cached = CaptureState {
            buffer: vk::Buffer::from_raw(1),
            byte_len: super::capture_byte_len(1280, 720, 4).expect("the cached size"),
            width: 1280,
            height: 720,
            format: vk::Format::B8G8R8A8_UNORM,
            ..CaptureState::dead()
        };
        // The capability loss with an otherwise perfect match: the
        // guard must refuse.
        assert!(
            !cached.matches(1280, 720, vk::Format::B8G8R8A8_UNORM, false),
            "a swapchain without TRANSFER_SRC must never be served from the \
             cache, however well it matches"
        );
        // With the capability present, the same match serves (the
        // positive control).
        assert!(cached.matches(1280, 720, vk::Format::B8G8R8A8_UNORM, true));
    }
}
