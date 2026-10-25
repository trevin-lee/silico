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

use worker::AdapterPolicy;

/// The first CPU adapter (`wgpu::DeviceType::Cpu`, lavapipe here) on Vulkan or Metal, or why
/// there's none.
pub fn cpu_adapter() -> Result<wgpu::Adapter, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let names: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();
    adapters
        .into_iter()
        .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
        .ok_or_else(|| {
            format!(
                "no software rasterizer was found (lavapipe's ICD: {:?}); the adapters found: {names:?}",
                lavapipe::ICD
            )
        })
}

/// The adapter `policy` allows: under [`AdapterPolicy::Software`], [`cpu_adapter`]; under
/// [`AdapterPolicy::Any`], a CPU adapter if there is one, then a discrete GPU, then any.
pub fn adapter(policy: AdapterPolicy) -> Result<wgpu::Adapter, String> {
    if policy == AdapterPolicy::Software {
        return cpu_adapter();
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let rank = |a: &wgpu::Adapter| match a.get_info().device_type {
        wgpu::DeviceType::Cpu => 0,
        wgpu::DeviceType::DiscreteGpu => 1,
        _ => 2,
    };
    adapters.sort_by_key(rank);
    let names: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();
    adapters.into_iter().next().ok_or_else(|| {
        format!("no adapter was found on Vulkan or Metal (lavapipe's ICD: {:?}); the adapters: {names:?}", lavapipe::ICD)
    })
}
