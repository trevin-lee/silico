//! Silico's rendering, on Rerun's `re_renderer` (docs/design/m4.6-render.md): the render
//! worker, which draws the core's scenes headless on Mesa's lavapipe and reads back their
//! image and ID buffer ([`worker`], [`render`]). The viewport (M5) grows from here.
//!
//! A render process takes lavapipe from the pixi environment it was built in, never from
//! ambient environment: [`lavapipe`] holds the paths the build found, and
//! [`lavapipe::reexec_with`] restarts the process with them set before anything touches
//! `wgpu`.

#[cfg(feature = "bench")]
pub mod bench;
pub mod lavapipe;
pub mod render;
pub mod worker;

use worker::RenderAdapter;

/// The scene sizes the test worker (`silico-render-test-worker`) misbehaves at, named once
/// for it and the tests: the core's client sends nothing but renders, so sizes no render
/// needs ask for each misbehaviour. Any other scene it draws as the worker does.
#[cfg(feature = "test_ops")]
pub mod test_scenes {
    /// Hangs for 600 s: a render past its time.
    pub const HANGS: (u32, u32) = (1, 7);
    /// Aborts: a worker that crashes.
    pub const CRASHES: (u32, u32) = (1, 9);
    /// Draws, and answers with one ID fewer than the scene's pixels.
    pub const IDS_SHORT: (u32, u32) = (1, 11);
    /// Answers with an empty PNG.
    pub const NO_IMAGE: (u32, u32) = (1, 13);
    /// Answers with the wrong kind of reply (an echo).
    pub const WRONG_REPLY: (u32, u32) = (1, 15);
    /// Draws, and answers with an ID buffer when none was asked for, or none when one was.
    pub const IDS_FLIPPED: (u32, u32) = (1, 17);
    /// Refuses the scene as invalid, as the worker refuses one that fails its check.
    pub const REFUSES: (u32, u32) = (1, 19);
    /// Refuses as having no adapter, its cause naming the policy the worker was started
    /// with (`policy Software` or `policy Any`).
    pub const SAYS_POLICY: (u32, u32) = (1, 21);
}

/// Every adapter on Vulkan or Metal (lavapipe among them where the worker's environment
/// names it), and their names, for a refusal to list.
fn all_adapters() -> (Vec<wgpu::Adapter>, Vec<String>) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let names = adapters.iter().map(|a| a.get_info().name).collect();
    (adapters, names)
}

/// The adapters `policy` allows, best first: under [`RenderAdapter::Software`], the CPU
/// adapters (`wgpu::DeviceType::Cpu`, lavapipe here); under [`RenderAdapter::Any`], those,
/// then discrete GPUs, then the rest. Or why there's none.
pub fn adapters(policy: RenderAdapter) -> Result<Vec<wgpu::Adapter>, String> {
    let (mut adapters, names) = all_adapters();
    let rank = |a: &wgpu::Adapter| match a.get_info().device_type {
        wgpu::DeviceType::Cpu => 0,
        wgpu::DeviceType::DiscreteGpu => 1,
        _ => 2,
    };
    adapters.sort_by_key(rank);
    adapters.retain(|a| {
        policy == RenderAdapter::Any || a.get_info().device_type == wgpu::DeviceType::Cpu
    });
    if adapters.is_empty() {
        return Err(match policy {
            RenderAdapter::Software => format!(
                "no software rasterizer was found (lavapipe's ICD: {:?}); the adapters found: {names:?}",
                lavapipe::ICD
            ),
            RenderAdapter::Any => format!(
                "no adapter was found on Vulkan or Metal (lavapipe's ICD: {:?})",
                lavapipe::ICD
            ),
        });
    }
    Ok(adapters)
}
