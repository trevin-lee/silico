//! Silico's rendering, on Rerun's `re_renderer` (docs/design/m4.6-render.md): for now, what
//! M4.6's first step proves, that a scene renders headless on Mesa's lavapipe and its picking
//! layer, the ID buffer, reads back. The render worker and the viewport (M5) grow from here.
//!
//! A render process takes lavapipe from the pixi environment it was built in, never from
//! ambient environment: [`lavapipe`] holds the paths the build found, and
//! [`lavapipe::reexec_with`] restarts the process with them set before anything touches
//! `wgpu`.

pub mod lavapipe;
pub mod probe;

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
