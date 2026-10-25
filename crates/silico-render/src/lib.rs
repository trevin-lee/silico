//! Silico's rendering, on Rerun's `re_renderer` (docs/design/m4.6-render.md): the render
//! worker, which draws the core's scenes headless on Mesa's lavapipe and reads back their
//! image and ID buffer ([`worker`], [`render`]). The viewport (M5) grows from here.
//!
//! A render process takes lavapipe from the pixi environment it was built in, never from
//! ambient environment: [`lavapipe`] holds the paths the build found, and
//! [`lavapipe::reexec_with`] restarts the process with them set before anything touches
//! `wgpu`.

pub mod bench;
pub mod lavapipe;
pub mod render;
pub mod worker;

use worker::RenderAdapter;

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
