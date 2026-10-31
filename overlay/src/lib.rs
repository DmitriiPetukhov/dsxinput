#![deny(unsafe_op_in_unsafe_fn)]

//! Vulkan layer implementation for the dsxinput in-game version overlay.

mod dispatch;
mod overlay_render;
mod render_sync;
mod swapchain;
