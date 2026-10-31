//! Lazy per-swapchain egui resources and batched Vulkan rendering.

use crate::{
    dispatch,
    render_sync::{BatchWaits, ImageSync},
    swapchain::{self, SwapchainState},
};
use ash::vk::{self, Handle};
use egui_ash_renderer::{
    allocator::DefaultAllocator, Options, RenderMode, Renderer as EguiRenderer,
};
use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
};

type RendererMap = HashMap<vk::SwapchainKHR, Arc<Mutex<SwapchainRenderer>>>;

static RENDERERS: OnceLock<Mutex<RendererMap>> = OnceLock::new();
static LOAD_REPORTED: AtomicBool = AtomicBool::new(false);
static FIRST_DRAW_REPORTED: AtomicBool = AtomicBool::new(false);

fn renderers() -> MutexGuard<'static, RendererMap> {
    RENDERERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct SubmissionFence {
    device: ash::Device,
    fence: vk::Fence,
}

impl SubmissionFence {
    fn wait(&self) -> Result<(), vk::Result> {
        unsafe { self.device.wait_for_fences(&[self.fence], true, u64::MAX) }
    }
}

impl Drop for SubmissionFence {
    fn drop(&mut self) {
        // Normal frames are explicitly waited before their Arc is replaced. Renderers
        // removed during teardown are first covered by device_wait_idle.
        unsafe { self.device.destroy_fence(self.fence, None) };
    }
}

struct LastSubmission {
    fence: Arc<SubmissionFence>,
    image_index: usize,
}

struct ImageTarget {
    image: vk::Image,
    view: vk::ImageView,
    framebuffer: vk::Framebuffer,
    output_semaphore: vk::Semaphore,
    sync: ImageSync,
}

impl ImageTarget {
    unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            device.destroy_semaphore(self.output_semaphore, None);
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_image_view(self.view, None);
        }
    }
}

struct FamilyCommands {
    pool: vk::CommandPool,
    buffers: Vec<vk::CommandBuffer>,
}

struct SwapchainRenderer {
    handle: vk::SwapchainKHR,
    device: vk::Device,
    ash_device: ash::Device,
    format: vk::Format,
    extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    images: Vec<ImageTarget>,
    family_commands: HashMap<u32, FamilyCommands>,
    egui: egui::Context,
    renderer: Option<EguiRenderer<DefaultAllocator>>,
    pending_texture_frees: Vec<egui::TextureId>,
    last_submission: Option<LastSubmission>,
    enabled: bool,
}

impl SwapchainRenderer {
    fn new(handle: vk::SwapchainKHR, state: &SwapchainState) -> Result<Self, String> {
        if state.images.is_empty() || state.extent.width == 0 || state.extent.height == 0 {
            return Err("swapchain images or extent are unavailable".into());
        }
        let context = dispatch::device_surface_context(state.device)
            .ok_or_else(|| "device dispatch state is unavailable".to_owned())?;
        let next_gipa = dispatch::next_instance_proc_addr(context.instance)
            .ok_or_else(|| "instance dispatch is unavailable".to_owned())?;
        let next_gdpa = dispatch::next_device_proc_addr(state.device)
            .ok_or_else(|| "device proc-address function is unavailable".to_owned())?;

        let instance = unsafe {
            ash::Instance::load_with(
                |name| function_pointer(next_gipa(context.instance, name.as_ptr())),
                context.instance,
            )
        };
        let device = unsafe {
            ash::Device::load_with(
                |name| function_pointer(next_gdpa(state.device, name.as_ptr())),
                state.device,
            )
        };

        let render_pass = create_render_pass(&device, state.format)?;
        let mut images = Vec::with_capacity(state.images.len());
        for &image in &state.images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(state.format)
                .components(vk::ComponentMapping::default())
                .subresource_range(color_subresource_range());
            let view = match unsafe { device.create_image_view(&view_info, None) } {
                Ok(view) => view,
                Err(error) => {
                    destroy_targets(&device, &mut images, render_pass);
                    return Err(format!("image view creation failed: {error:?}"));
                }
            };
            let attachments = [view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(state.extent.width)
                .height(state.extent.height)
                .layers(1);
            let framebuffer = match unsafe { device.create_framebuffer(&framebuffer_info, None) } {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    unsafe { device.destroy_image_view(view, None) };
                    destroy_targets(&device, &mut images, render_pass);
                    return Err(format!("framebuffer creation failed: {error:?}"));
                }
            };
            let semaphore =
                match unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                {
                    Ok(semaphore) => semaphore,
                    Err(error) => {
                        unsafe {
                            device.destroy_framebuffer(framebuffer, None);
                            device.destroy_image_view(view, None);
                        }
                        destroy_targets(&device, &mut images, render_pass);
                        return Err(format!("output semaphore creation failed: {error:?}"));
                    }
                };
            images.push(ImageTarget {
                image,
                view,
                framebuffer,
                output_semaphore: semaphore,
                sync: ImageSync::new(),
            });
        }

        let options = Options {
            in_flight_frames: 1,
            srgb_framebuffer: is_srgb_format(state.format),
            ..Options::default()
        };
        let renderer = match EguiRenderer::with_default_allocator(
            &instance,
            context.physical_device,
            device.clone(),
            RenderMode::RenderPass(render_pass),
            options,
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                destroy_targets(&device, &mut images, render_pass);
                return Err(format!("egui renderer creation failed: {error}"));
            }
        };

        Ok(Self {
            handle,
            device: state.device,
            ash_device: device,
            format: state.format,
            extent: state.extent,
            render_pass,
            images,
            family_commands: HashMap::new(),
            egui: egui::Context::default(),
            renderer: Some(renderer),
            pending_texture_frees: Vec::new(),
            last_submission: None,
            enabled: true,
        })
    }

    fn matches(&self, state: &SwapchainState) -> bool {
        self.format == state.format
            && self.extent == state.extent
            && self.images.len() == state.images.len()
            && self
                .images
                .iter()
                .zip(&state.images)
                .all(|(target, image)| target.image == *image)
    }

    fn ensure_acquired(&mut self, index: usize) -> bool {
        let Some(image) = self.images.get_mut(index) else {
            return false;
        };
        image.sync.is_acquired() || image.sync.acquired()
    }

    fn wait_previous_submission(&mut self) -> Result<(), String> {
        let Some(previous) = self.last_submission.as_ref() else {
            return Ok(());
        };
        let image_index = previous.image_index;
        let fence = Arc::clone(&previous.fence);
        match fence.wait() {
            Ok(()) => {
                self.last_submission.take();
                if let Some(image) = self.images.get_mut(image_index) {
                    image.sync.fence_complete();
                }
                Ok(())
            }
            Err(error) => {
                if let Some(image) = self.images.get_mut(image_index) {
                    image.sync.submission_failed();
                }
                Err(format!(
                    "previous overlay submission fence failed: {error:?}"
                ))
            }
        }
    }

    fn command_buffer(
        &mut self,
        family_index: u32,
        image_index: usize,
    ) -> Result<(vk::CommandPool, vk::CommandBuffer), String> {
        if !self.family_commands.contains_key(&family_index) {
            let pool_info = vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                .queue_family_index(family_index);
            let pool = unsafe { self.ash_device.create_command_pool(&pool_info, None) }
                .map_err(|error| format!("command-pool creation failed: {error:?}"))?;
            let allocate_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(self.images.len() as u32);
            let buffers = match unsafe { self.ash_device.allocate_command_buffers(&allocate_info) }
            {
                Ok(buffers) => buffers,
                Err(error) => {
                    unsafe { self.ash_device.destroy_command_pool(pool, None) };
                    return Err(format!("command-buffer allocation failed: {error:?}"));
                }
            };
            self.family_commands
                .insert(family_index, FamilyCommands { pool, buffers });
        }
        let family = self.family_commands.get(&family_index).unwrap();
        let command_buffer = *family
            .buffers
            .get(image_index)
            .ok_or_else(|| "command buffer index is unavailable".to_owned())?;
        unsafe {
            self.ash_device
                .reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty())
        }
        .map_err(|error| format!("command-buffer reset failed: {error:?}"))?;
        Ok((family.pool, command_buffer))
    }

    fn record_frame(
        &mut self,
        queue: vk::Queue,
        family_index: u32,
        image_index: usize,
    ) -> Result<(vk::CommandBuffer, vk::Semaphore), String> {
        if !self.images[image_index].sync.command_reusable()
            || !self.images[image_index].sync.output_reusable()
        {
            return Err("overlay image synchronization is not reusable".into());
        }
        let (pool, command_buffer) = self.command_buffer(family_index, image_index)?;

        if let Some(renderer) = self.renderer.as_mut() {
            for texture_id in self.pending_texture_frees.drain(..) {
                renderer
                    .free_texture(texture_id)
                    .map_err(|error| format!("texture release failed: {error}"))?;
            }
        }

        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.extent.width as f32, self.extent.height as f32),
        ));
        let mut output = self.egui.run_ui(input, |root_ui| {
            egui::Area::new(egui::Id::new("dsxinput-version-overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(12.0, 12.0))
                .interactable(false)
                .show(root_ui.ctx(), |ui| {
                    egui::Frame::default()
                        .fill(egui::Color32::from_black_alpha(160))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "dsxinput v{}",
                                    env!("CARGO_PKG_VERSION")
                                ))
                                .color(egui::Color32::WHITE),
                            );
                        });
                });
        });

        let mut texture_delta = std::mem::take(&mut output.textures_delta);
        let texture_frees = texture_delta.free.iter().copied().collect::<Vec<_>>();
        let mut texture_update_error = None;
        if let Some(renderer) = self.renderer.as_mut() {
            'textures: for (texture_id, deltas) in &texture_delta.set {
                for delta in deltas {
                    if let Err(error) = renderer.set_texture(queue, pool, *texture_id, delta) {
                        texture_update_error = Some(format!("texture update failed: {error}"));
                        break 'textures;
                    }
                }
            }
        } else {
            texture_update_error = Some("egui renderer is unavailable".to_owned());
        }
        texture_delta.clear();
        self.pending_texture_frees.extend(texture_frees);
        if let Some(error) = texture_update_error {
            return Err(error);
        }
        let primitives = self.egui.tessellate(output.shapes, output.pixels_per_point);
        let begin_info = vk::CommandBufferBeginInfo::default();
        unsafe {
            self.ash_device
                .begin_command_buffer(command_buffer, &begin_info)
        }
        .map_err(|error| format!("command-buffer begin failed: {error:?}"))?;

        let image = self.images[image_index].image;
        let subresource = color_subresource_range();
        let before = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )
            .old_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(subresource);
        unsafe {
            self.ash_device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[before],
            )
        };

        let render_area = vk::Rect2D {
            offset: vk::Offset2D::default(),
            extent: self.extent,
        };
        let begin_render_pass = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(self.images[image_index].framebuffer)
            .render_area(render_area);
        unsafe {
            self.ash_device.cmd_begin_render_pass(
                command_buffer,
                &begin_render_pass,
                vk::SubpassContents::INLINE,
            )
        };
        self.renderer
            .as_mut()
            .ok_or_else(|| "egui renderer is unavailable".to_owned())?
            .cmd_draw(
                command_buffer,
                self.extent,
                output.pixels_per_point,
                &primitives,
            )
            .map_err(|error| format!("egui command recording failed: {error}"))?;
        unsafe { self.ash_device.cmd_end_render_pass(command_buffer) };

        let after = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(subresource);
        unsafe {
            self.ash_device.cmd_pipeline_barrier(
                command_buffer,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[after],
            )
        };
        unsafe { self.ash_device.end_command_buffer(command_buffer) }
            .map_err(|error| format!("command-buffer end failed: {error:?}"))?;

        Ok((command_buffer, self.images[image_index].output_semaphore))
    }

    fn set_last_submission(&mut self, fence: Arc<SubmissionFence>, image_index: usize) {
        self.last_submission = Some(LastSubmission { fence, image_index });
    }

    fn disable(&mut self, reason: &'static str) {
        self.enabled = false;
        swapchain::disable_for_render(self.handle, reason);
    }
}

impl Drop for SwapchainRenderer {
    fn drop(&mut self) {
        self.last_submission.take();
        drop(self.renderer.take());
        for (_, commands) in self.family_commands.drain() {
            unsafe { self.ash_device.destroy_command_pool(commands.pool, None) };
        }
        for mut image in self.images.drain(..) {
            unsafe { image.destroy(&self.ash_device) };
        }
        unsafe { self.ash_device.destroy_render_pass(self.render_pass, None) };
    }
}

struct PreparedImage {
    handle: vk::SwapchainKHR,
    image_index: usize,
    command_buffer: vk::CommandBuffer,
    output_semaphore: vk::Semaphore,
}

/// Forwards one present call, optionally submitting all eligible overlay draws first.
pub(crate) unsafe fn queue_present(
    queue: vk::Queue,
    present_info: *const vk::PresentInfoKHR<'_>,
    present: vk::PFN_vkQueuePresentKHR,
) -> vk::Result {
    if present_info.is_null() {
        return unsafe { present(queue, present_info) };
    }
    let info = unsafe { &*present_info };
    if info.swapchain_count == 0
        || info.p_swapchains.is_null()
        || info.p_image_indices.is_null()
        || (info.wait_semaphore_count > 0 && info.p_wait_semaphores.is_null())
    {
        let result = unsafe { present(queue, present_info) };
        swapchain::record_present_results(present_info, result);
        return result;
    }
    let Some(queue_metadata) = dispatch::queue_metadata(queue) else {
        let result = unsafe { present(queue, present_info) };
        swapchain::record_present_results(present_info, result);
        return result;
    };

    let swapchains =
        unsafe { std::slice::from_raw_parts(info.p_swapchains, info.swapchain_count as usize) };
    let image_indices =
        unsafe { std::slice::from_raw_parts(info.p_image_indices, info.swapchain_count as usize) };
    if swapchains.iter().copied().collect::<HashSet<_>>().len() != swapchains.len() {
        let result = unsafe { present(queue, present_info) };
        swapchain::record_present_results(present_info, result);
        return result;
    }

    let mut candidates = Vec::new();
    for (handle, image_index) in swapchains
        .iter()
        .copied()
        .zip(image_indices.iter().copied())
    {
        let Some(state) = swapchain::state(handle) else {
            continue;
        };
        if !state.enabled
            || !state.acquired_images.contains(&image_index)
            || image_index as usize >= state.images.len()
            || !swapchain::is_eligible_for_queue(handle, queue)
        {
            continue;
        }
        let renderer = match get_or_create_renderer(handle, &state) {
            Ok(renderer) => renderer,
            Err(_error) => {
                swapchain::disable_for_render(handle, "swapchain renderer initialization failed");
                continue;
            }
        };
        candidates.push((handle, image_index as usize, renderer));
    }
    candidates.sort_by_key(|(handle, _, _)| handle.as_raw());

    let mut guards = candidates
        .iter()
        .map(|(_, _, renderer)| {
            renderer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        })
        .collect::<Vec<_>>();
    let mut prepared = Vec::new();
    for (candidate_index, ((handle, image_index, _), renderer)) in
        candidates.iter().zip(guards.iter_mut()).enumerate()
    {
        if !renderer.enabled || !renderer.ensure_acquired(*image_index) {
            continue;
        }
        if renderer.wait_previous_submission().is_err() {
            renderer.disable("overlay previous submission fence wait failed");
            continue;
        }
        if !renderer.images[*image_index].sync.can_submit() {
            continue;
        }
        match renderer.record_frame(queue, queue_metadata.family_index, *image_index) {
            Ok((command_buffer, output_semaphore)) => {
                if renderer.images[*image_index].sync.begin_submission() {
                    prepared.push((
                        candidate_index,
                        PreparedImage {
                            handle: *handle,
                            image_index: *image_index,
                            command_buffer,
                            output_semaphore,
                        },
                    ));
                } else {
                    renderer.disable("overlay image synchronization could not be prepared");
                }
            }
            Err(_error) => {
                renderer.disable("overlay command recording failed");
            }
        }
    }

    if prepared.is_empty() {
        let result = unsafe { present(queue, present_info) };
        update_presented_images(present_info, result, &[], &mut guards);
        swapchain::record_present_results(present_info, result);
        return result;
    }

    let device = guards[prepared[0].0].ash_device.clone();
    let fence = match unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) } {
        Ok(fence) => fence,
        Err(_error) => {
            for (candidate_index, image) in &prepared {
                let renderer = &mut guards[*candidate_index];
                renderer.images[image.image_index].sync.submission_failed();
                renderer.disable("overlay submission fence creation failed");
            }
            let result = unsafe { present(queue, present_info) };
            update_presented_images(present_info, result, &[], &mut guards);
            swapchain::record_present_results(present_info, result);
            return result;
        }
    };

    let input_waits = if info.wait_semaphore_count == 0 {
        &[][..]
    } else {
        unsafe {
            std::slice::from_raw_parts(info.p_wait_semaphores, info.wait_semaphore_count as usize)
        }
    };
    let mut batch_waits = BatchWaits::new(input_waits);
    let wait_semaphores = batch_waits
        .take_for_submission()
        .expect("each present batch has one wait-semaphore submission");
    let wait_stages = vec![vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT; wait_semaphores.len()];
    let command_buffers = prepared
        .iter()
        .map(|(_, image)| image.command_buffer)
        .collect::<Vec<_>>();
    let signal_semaphores = prepared
        .iter()
        .map(|(_, image)| image.output_semaphore)
        .collect::<Vec<_>>();
    let submit = vk::SubmitInfo::default()
        .wait_semaphores(wait_semaphores)
        .wait_dst_stage_mask(&wait_stages)
        .command_buffers(&command_buffers)
        .signal_semaphores(&signal_semaphores);
    match unsafe { device.queue_submit(queue, &[submit], fence) } {
        Ok(()) => {
            let fence = Arc::new(SubmissionFence {
                device: device.clone(),
                fence,
            });
            for (candidate_index, image) in &prepared {
                guards[*candidate_index].set_last_submission(Arc::clone(&fence), image.image_index);
            }
            report_loaded_once();
        }
        Err(error) => {
            // Keep the fence alive until device-idle teardown. DEVICE_LOST may
            // still leave this submission associated with pending queue work.
            let fence = Arc::new(SubmissionFence {
                device: device.clone(),
                fence,
            });
            for (candidate_index, image) in &prepared {
                let renderer = &mut guards[*candidate_index];
                renderer.images[image.image_index].sync.submission_failed();
                renderer.set_last_submission(Arc::clone(&fence), image.image_index);
                renderer.disable("overlay queue submission failed; synchronization is poisoned");
            }
            drop(fence);
            unsafe { write_failed_present_results(present_info, error) };
            swapchain::record_present_results(present_info, error);
            return error;
        }
    }

    let mut forwarded = unsafe { *present_info };
    forwarded.wait_semaphore_count = signal_semaphores.len() as u32;
    forwarded.p_wait_semaphores = signal_semaphores.as_ptr();
    let result = unsafe { present(queue, &forwarded) };
    update_presented_images(present_info, result, &prepared, &mut guards);
    swapchain::record_present_results(present_info, result);
    result
}

unsafe fn write_failed_present_results(
    present_info: *const vk::PresentInfoKHR<'_>,
    result: vk::Result,
) {
    if present_info.is_null() {
        return;
    }
    let info = unsafe { &*present_info };
    if info.p_results.is_null() {
        return;
    }
    for index in 0..info.swapchain_count as usize {
        unsafe { *info.p_results.add(index) = result };
    }
}

fn update_presented_images(
    present_info: *const vk::PresentInfoKHR<'_>,
    result: vk::Result,
    prepared: &[(usize, PreparedImage)],
    guards: &mut [MutexGuard<'_, SwapchainRenderer>],
) {
    if present_info.is_null() {
        return;
    }
    let info = unsafe { &*present_info };
    if info.p_swapchains.is_null() || info.p_image_indices.is_null() {
        return;
    }
    let rendered = prepared
        .iter()
        .map(|(_, image)| (image.handle, image.image_index as u32))
        .collect::<HashSet<_>>();
    for index in 0..info.swapchain_count as usize {
        let handle = unsafe { *info.p_swapchains.add(index) };
        let image_index = unsafe { *info.p_image_indices.add(index) };
        let image_result = if info.p_results.is_null() {
            result
        } else {
            unsafe { *info.p_results.add(index) }
        };
        let Some(renderer) = guards.iter_mut().find(|renderer| renderer.handle == handle) else {
            continue;
        };
        let Some(image) = renderer.images.get_mut(image_index as usize) else {
            continue;
        };
        if rendered.contains(&(handle, image_index as u32)) {
            if !image.sync.presented(image_result) {
                renderer.disable("overlay presentation failed; synchronization is poisoned");
            } else if !FIRST_DRAW_REPORTED.swap(true, Ordering::Relaxed) {
                eprintln!("dsxinput-overlay: first overlay frame presented");
            }
        } else {
            image.sync.presented_without_submission(image_result);
        }
    }
}

fn get_or_create_renderer(
    handle: vk::SwapchainKHR,
    state: &SwapchainState,
) -> Result<Arc<Mutex<SwapchainRenderer>>, String> {
    if let Some(renderer) = renderers().get(&handle).cloned() {
        let matches = renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .matches(state);
        if !matches {
            return Err("existing renderer does not match swapchain images".into());
        }
        return Ok(renderer);
    }
    let created = Arc::new(Mutex::new(SwapchainRenderer::new(handle, state)?));
    let mut registry = renderers();
    if let Some(existing) = registry.get(&handle) {
        return Ok(Arc::clone(existing));
    }
    registry.insert(handle, Arc::clone(&created));
    Ok(created)
}

fn function_pointer(function: vk::PFN_vkVoidFunction) -> *const c_void {
    function.map_or(ptr::null(), |function| {
        function as *const () as *const c_void
    })
}

#[cfg(test)]
mod tests {
    use super::write_failed_present_results;
    use ash::vk::{self, Handle};

    #[test]
    fn failed_overlay_submission_initializes_each_present_result() {
        let swapchains = [
            vk::SwapchainKHR::from_raw(0x701),
            vk::SwapchainKHR::from_raw(0x702),
        ];
        let image_indices = [0, 1];
        let mut results = [vk::Result::NOT_READY; 2];
        let mut info = vk::PresentInfoKHR::default()
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        info.p_results = results.as_mut_ptr();

        unsafe {
            write_failed_present_results(&info, vk::Result::ERROR_DEVICE_LOST);
        }

        assert_eq!(
            results,
            [vk::Result::ERROR_DEVICE_LOST, vk::Result::ERROR_DEVICE_LOST]
        );
    }
}

fn color_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .base_mip_level(0)
        .level_count(1)
        .base_array_layer(0)
        .layer_count(1)
}

fn create_render_pass(device: &ash::Device, format: vk::Format) -> Result<vk::RenderPass, String> {
    let attachments = [vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::LOAD)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let color_attachments = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_attachments)];
    let dependencies = [
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            ),
        vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::BOTTOM_OF_PIPE)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::empty()),
    ];
    let create_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&dependencies);
    unsafe { device.create_render_pass(&create_info, None) }
        .map_err(|error| format!("render-pass creation failed: {error:?}"))
}

fn destroy_targets(
    device: &ash::Device,
    images: &mut Vec<ImageTarget>,
    render_pass: vk::RenderPass,
) {
    for mut image in images.drain(..) {
        unsafe { image.destroy(device) };
    }
    unsafe { device.destroy_render_pass(render_pass, None) };
}

fn is_srgb_format(format: vk::Format) -> bool {
    matches!(
        format,
        vk::Format::R8G8B8_SRGB
            | vk::Format::B8G8R8_SRGB
            | vk::Format::R8G8B8A8_SRGB
            | vk::Format::B8G8R8A8_SRGB
            | vk::Format::A8B8G8R8_SRGB_PACK32
    )
}

fn report_loaded_once() {
    if !LOAD_REPORTED.swap(true, Ordering::Relaxed) {
        eprintln!("dsxinput-overlay: Vulkan overlay loaded");
    }
}

pub(crate) fn record_acquired(handle: vk::SwapchainKHR, image_index: u32) {
    let renderer = renderers().get(&handle).cloned();
    let Some(renderer) = renderer else {
        return;
    };
    let mut renderer = renderer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(image) = renderer.images.get_mut(image_index as usize) else {
        return;
    };
    if !image.sync.acquired() {
        renderer.disable("overlay image acquire state is inconsistent");
    }
}

pub(crate) fn remove_swapchain(handle: vk::SwapchainKHR) {
    let renderer = renderers().remove(&handle);
    if let Some(renderer_arc) = renderer {
        let mut renderer = renderer_arc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        renderer.enabled = false;
        let _ = unsafe { renderer.ash_device.device_wait_idle() };
        for image in &mut renderer.images {
            image.sync.device_idle();
            debug_assert!(image.sync.teardown_ready());
        }
        drop(renderer);
        drop(renderer_arc);
    }
}

pub(crate) fn remove_device(device: vk::Device) {
    let removed = take_renderers_for_device(device);
    if let Some(renderer) = removed.first() {
        let renderer = renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = unsafe { renderer.ash_device.device_wait_idle() };
    }
    for renderer in &removed {
        let mut renderer = renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for image in &mut renderer.images {
            image.sync.device_idle();
            debug_assert!(image.sync.teardown_ready());
        }
    }
    drop(removed);
}

pub(crate) fn remove_instance(instance: vk::Instance) {
    let snapshot = renderers()
        .iter()
        .map(|(handle, renderer)| (*handle, Arc::clone(renderer)))
        .collect::<Vec<_>>();
    let mut devices = HashSet::new();
    for (_, renderer) in snapshot {
        let device = renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .device;
        if dispatch::device_surface_context(device)
            .is_some_and(|context| context.instance == instance)
        {
            devices.insert(device);
        }
    }
    for device in devices {
        dispatch::with_device_queue_serialization(device, || remove_device(device));
    }
}

fn take_renderers_for_device(device: vk::Device) -> Vec<Arc<Mutex<SwapchainRenderer>>> {
    let snapshot = renderers()
        .iter()
        .map(|(handle, renderer)| (*handle, Arc::clone(renderer)))
        .collect::<Vec<_>>();
    let handles = snapshot
        .into_iter()
        .filter_map(|(handle, renderer)| {
            let belongs = renderer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .device
                == device;
            belongs.then_some(handle)
        })
        .collect::<Vec<_>>();
    let mut registry = renderers();
    handles
        .into_iter()
        .filter_map(|handle| registry.remove(&handle))
        .collect()
}
