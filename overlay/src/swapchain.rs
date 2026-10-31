//! Swapchain eligibility, acquisition state, and Vulkan extension forwarding.

use crate::{
    dispatch,
    hdr::{classify_output_color_mode, OutputColorMode},
};
use ash::vk::{self, Handle};
use std::{
    collections::{HashMap, HashSet},
    ffi::CStr,
    panic::AssertUnwindSafe,
    sync::{Mutex, MutexGuard, OnceLock},
};

#[derive(Clone)]
pub(crate) struct SwapchainState {
    pub(crate) device: vk::Device,
    instance: vk::Instance,
    pub(crate) format: vk::Format,
    pub(crate) color_space: vk::ColorSpaceKHR,
    pub(crate) output_color_mode: Option<OutputColorMode>,
    pub(crate) image_usage: vk::ImageUsageFlags,
    pub(crate) extent: vk::Extent2D,
    pub(crate) enabled: bool,
    pub(crate) sharing_mode: vk::SharingMode,
    pub(crate) queue_family_indices: Vec<u32>,
    pub(crate) present_families: HashSet<u32>,
    pub(crate) images: Vec<vk::Image>,
    pub(crate) acquired_images: HashSet<u32>,
    pub(crate) presented_images: HashSet<u32>,
    pub(crate) uncertain_images: HashSet<u32>,
}

static SWAPCHAINS: OnceLock<Mutex<HashMap<vk::SwapchainKHR, SwapchainState>>> = OnceLock::new();
static REPORTED_DISABLE_REASONS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();

fn swapchains() -> MutexGuard<'static, HashMap<vk::SwapchainKHR, SwapchainState>> {
    SWAPCHAINS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn report_disabled_once(reason: &'static str) {
    let mut reported = REPORTED_DISABLE_REASONS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if reported.insert(reason) {
        eprintln!("dsxinput-overlay: overlay disabled: {reason}");
    }
}

pub(crate) fn disable_for_render(swapchain: vk::SwapchainKHR, reason: &'static str) {
    if let Some(state) = swapchains().get_mut(&swapchain) {
        state.enabled = false;
    }
    report_disabled_once(reason);
}

pub(crate) fn state(swapchain: vk::SwapchainKHR) -> Option<SwapchainState> {
    swapchains().get(&swapchain).cloned()
}

#[cfg(test)]
pub(crate) fn clear() {
    swapchains().clear();
}

fn remove(swapchain: vk::SwapchainKHR) {
    swapchains().remove(&swapchain);
}

pub(crate) fn remove_device(device: vk::Device) {
    swapchains().retain(|_, state| state.device != device);
}

pub(crate) fn remove_instance(instance: vk::Instance) {
    swapchains().retain(|_, state| state.instance != instance);
}

/// Reports whether the requested present queue is usable for rendering this swapchain.
#[allow(dead_code)] // Consumed by the rendering step once its submission path is added.
pub(crate) fn is_eligible_for_queue(swapchain: vk::SwapchainKHR, queue: vk::Queue) -> bool {
    let Some(state) = state(swapchain) else {
        return false;
    };
    if !state.enabled {
        return false;
    }
    let Some(queue) = dispatch::queue_metadata(queue) else {
        return false;
    };
    if queue.device != state.device
        || !queue.family_flags.contains(vk::QueueFlags::GRAPHICS)
        || queue.flags.contains(vk::DeviceQueueCreateFlags::PROTECTED)
        || !state.present_families.contains(&queue.family_index)
    {
        return false;
    }
    match state.sharing_mode {
        vk::SharingMode::EXCLUSIVE => true,
        vk::SharingMode::CONCURRENT => state.queue_family_indices.contains(&queue.family_index),
        _ => false,
    }
}

pub(crate) unsafe fn instance_proc(
    instance: vk::Instance,
    name: &CStr,
    next: Option<vk::PFN_vkGetInstanceProcAddr>,
) -> vk::PFN_vkVoidFunction {
    match name.to_bytes() {
        b"vkCreateSwapchainKHR" => erase_instance_if_supported(
            instance,
            name,
            next,
            vkCreateSwapchainKHR as vk::PFN_vkCreateSwapchainKHR,
        ),
        b"vkDestroySwapchainKHR" => erase_instance_if_supported(
            instance,
            name,
            next,
            vkDestroySwapchainKHR as vk::PFN_vkDestroySwapchainKHR,
        ),
        b"vkGetSwapchainImagesKHR" => erase_instance_if_supported(
            instance,
            name,
            next,
            vkGetSwapchainImagesKHR as vk::PFN_vkGetSwapchainImagesKHR,
        ),
        b"vkAcquireNextImageKHR" => erase_instance_if_supported(
            instance,
            name,
            next,
            vkAcquireNextImageKHR as vk::PFN_vkAcquireNextImageKHR,
        ),
        b"vkAcquireNextImage2KHR" => erase_instance_if_supported(
            instance,
            name,
            next,
            vkAcquireNextImage2KHR as vk::PFN_vkAcquireNextImage2KHR,
        ),
        b"vkQueuePresentKHR" => erase_instance_if_supported(
            instance,
            name,
            next,
            vkQueuePresentKHR as vk::PFN_vkQueuePresentKHR,
        ),
        _ => None,
    }
}

fn erase_instance_if_supported<T: Copy>(
    instance: vk::Instance,
    name: &CStr,
    next: Option<vk::PFN_vkGetInstanceProcAddr>,
    layer_function: T,
) -> vk::PFN_vkVoidFunction {
    let Some(next) = next else {
        return None;
    };
    if unsafe { next(instance, name.as_ptr()) }.is_none() {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy(&layer_function) })
}

pub(crate) unsafe fn device_proc(
    device: vk::Device,
    name: &CStr,
    next: Option<vk::PFN_vkGetDeviceProcAddr>,
) -> vk::PFN_vkVoidFunction {
    match name.to_bytes() {
        b"vkCreateSwapchainKHR" => erase_device_if_supported(
            device,
            name,
            next,
            vkCreateSwapchainKHR as vk::PFN_vkCreateSwapchainKHR,
        ),
        b"vkDestroySwapchainKHR" => erase_device_if_supported(
            device,
            name,
            next,
            vkDestroySwapchainKHR as vk::PFN_vkDestroySwapchainKHR,
        ),
        b"vkGetSwapchainImagesKHR" => erase_device_if_supported(
            device,
            name,
            next,
            vkGetSwapchainImagesKHR as vk::PFN_vkGetSwapchainImagesKHR,
        ),
        b"vkAcquireNextImageKHR" => erase_device_if_supported(
            device,
            name,
            next,
            vkAcquireNextImageKHR as vk::PFN_vkAcquireNextImageKHR,
        ),
        b"vkAcquireNextImage2KHR" => erase_device_if_supported(
            device,
            name,
            next,
            vkAcquireNextImage2KHR as vk::PFN_vkAcquireNextImage2KHR,
        ),
        b"vkQueuePresentKHR" => erase_device_if_supported(
            device,
            name,
            next,
            vkQueuePresentKHR as vk::PFN_vkQueuePresentKHR,
        ),
        _ => None,
    }
}

fn erase_device_if_supported<T: Copy>(
    device: vk::Device,
    name: &CStr,
    next: Option<vk::PFN_vkGetDeviceProcAddr>,
    layer_function: T,
) -> vk::PFN_vkVoidFunction {
    let Some(next) = next else {
        return None;
    };
    if unsafe { next(device, name.as_ptr()) }.is_none() {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy(&layer_function) })
}

#[no_mangle]
pub(crate) unsafe extern "system" fn vkCreateSwapchainKHR(
    device: vk::Device,
    create_info: *const vk::SwapchainCreateInfoKHR<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    swapchain: *mut vk::SwapchainKHR,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        create_swapchain(device, create_info, allocator, swapchain)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub(crate) unsafe extern "system" fn vkDestroySwapchainKHR(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    allocator: *const vk::AllocationCallbacks<'_>,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        destroy_swapchain(device, swapchain, allocator)
    }));
}

#[no_mangle]
pub(crate) unsafe extern "system" fn vkGetSwapchainImagesKHR(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    image_count: *mut u32,
    images: *mut vk::Image,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        get_swapchain_images(device, swapchain, image_count, images)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub(crate) unsafe extern "system" fn vkAcquireNextImageKHR(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    timeout: u64,
    semaphore: vk::Semaphore,
    fence: vk::Fence,
    image_index: *mut u32,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        acquire_next_image(device, swapchain, timeout, semaphore, fence, image_index)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub(crate) unsafe extern "system" fn vkAcquireNextImage2KHR(
    device: vk::Device,
    acquire_info: *const vk::AcquireNextImageInfoKHR<'_>,
    image_index: *mut u32,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        acquire_next_image2(device, acquire_info, image_index)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub(crate) unsafe extern "system" fn vkQueuePresentKHR(
    queue: vk::Queue,
    present_info: *const vk::PresentInfoKHR<'_>,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        queue_present(queue, present_info)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

struct Eligibility {
    output_color_mode: Option<OutputColorMode>,
    required_usage: vk::ImageUsageFlags,
    present_families: HashSet<u32>,
    disable_reason: Option<&'static str>,
}

unsafe fn evaluate_eligibility(
    device: vk::Device,
    create_info: &vk::SwapchainCreateInfoKHR<'_>,
) -> Eligibility {
    if create_info.image_array_layers != 1 {
        return disabled(
            "swapchain has more than one image layer",
            classify_output_color_mode(create_info.image_format, create_info.image_color_space),
        );
    }
    let output_color_mode =
        classify_output_color_mode(create_info.image_format, create_info.image_color_space);
    let Some(output_color_mode) = output_color_mode else {
        return disabled(
            "swapchain color space and format are unsupported",
            output_color_mode,
        );
    };
    if create_info
        .flags
        .contains(vk::SwapchainCreateFlagsKHR::PROTECTED)
    {
        return disabled(
            "protected swapchains are unsupported",
            Some(output_color_mode),
        );
    }
    if matches!(
        create_info.present_mode,
        vk::PresentModeKHR::SHARED_DEMAND_REFRESH | vk::PresentModeKHR::SHARED_CONTINUOUS_REFRESH
    ) {
        return disabled(
            "shared-present modes are unsupported",
            Some(output_color_mode),
        );
    }
    let queue_family_indices = match sharing_family_indices(create_info) {
        Ok(indices) => indices,
        Err(reason) => return disabled(reason, Some(output_color_mode)),
    };
    let Some(context) = dispatch::device_surface_context(device) else {
        return disabled(
            "device queue-family state is unavailable",
            Some(output_color_mode),
        );
    };
    let Some(capabilities) = (unsafe { surface_capabilities(&context, create_info.surface) })
    else {
        return disabled("surface capabilities query failed", Some(output_color_mode));
    };
    let required_usage = required_swapchain_usage(output_color_mode);
    if !capabilities.supported_usage_flags.contains(required_usage) {
        return disabled(
            match output_color_mode {
                OutputColorMode::Sdr => "surface does not support color-attachment images",
                OutputColorMode::Hdr10Pq | OutputColorMode::ScRgbLinear => {
                    "surface does not support HDR color-attachment and transfer-source images"
                }
            },
            Some(output_color_mode),
        );
    }
    if matches!(
        output_color_mode,
        OutputColorMode::Hdr10Pq | OutputColorMode::ScRgbLinear
    ) {
        if let Err(reason) =
            unsafe { validate_hdr_image_support(&context, create_info.image_format) }
        {
            return disabled(reason, Some(output_color_mode));
        }
    }
    let present_families = match unsafe {
        surface_present_families(&context, create_info.surface, &queue_family_indices)
    } {
        Ok(families) => families,
        Err(reason) => return disabled(reason, Some(output_color_mode)),
    };
    if present_families.is_empty() {
        return disabled(
            "no requested graphics queue can present to the surface",
            Some(output_color_mode),
        );
    }
    Eligibility {
        output_color_mode: Some(output_color_mode),
        required_usage,
        present_families,
        disable_reason: None,
    }
}

fn required_swapchain_usage(output_color_mode: OutputColorMode) -> vk::ImageUsageFlags {
    match output_color_mode {
        OutputColorMode::Sdr => vk::ImageUsageFlags::COLOR_ATTACHMENT,
        OutputColorMode::Hdr10Pq | OutputColorMode::ScRgbLinear => {
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC
        }
    }
}

fn disabled(reason: &'static str, output_color_mode: Option<OutputColorMode>) -> Eligibility {
    Eligibility {
        output_color_mode,
        required_usage: output_color_mode
            .map_or(vk::ImageUsageFlags::empty(), required_swapchain_usage),
        present_families: HashSet::new(),
        disable_reason: Some(reason),
    }
}

unsafe fn validate_hdr_image_support(
    context: &dispatch::DeviceSurfaceContext,
    swapchain_format: vk::Format,
) -> Result<(), &'static str> {
    let Some(next) = dispatch::next_instance_proc_addr(context.instance) else {
        return Err("instance dispatch is unavailable for HDR format queries");
    };
    let Some(image_format_function) = (unsafe {
        next(
            context.instance,
            c"vkGetPhysicalDeviceImageFormatProperties".as_ptr(),
        )
    }) else {
        return Err("HDR image-format query is unavailable");
    };
    let image_format_properties: vk::PFN_vkGetPhysicalDeviceImageFormatProperties =
        unsafe { std::mem::transmute_copy(&image_format_function) };
    for (format, usage, failure_reason) in [
        (
            swapchain_format,
            vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED,
            "swapchain format cannot be used for the HDR scene copy",
        ),
        (
            vk::Format::R16G16B16A16_SFLOAT,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            "float UI image usage is unsupported",
        ),
    ] {
        let mut properties = vk::ImageFormatProperties::default();
        let result = unsafe {
            image_format_properties(
                context.physical_device,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                usage,
                vk::ImageCreateFlags::empty(),
                &mut properties,
            )
        };
        if result != vk::Result::SUCCESS {
            return Err(failure_reason);
        }
    }

    let Some(format_function) = (unsafe {
        next(
            context.instance,
            c"vkGetPhysicalDeviceFormatProperties".as_ptr(),
        )
    }) else {
        return Err("HDR format-feature query is unavailable");
    };
    let format_properties: vk::PFN_vkGetPhysicalDeviceFormatProperties =
        unsafe { std::mem::transmute_copy(&format_function) };
    let mut properties = vk::FormatProperties::default();
    unsafe {
        format_properties(
            context.physical_device,
            vk::Format::R16G16B16A16_SFLOAT,
            &mut properties,
        )
    };
    let required_features = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE;
    if !properties
        .optimal_tiling_features
        .contains(required_features)
    {
        return Err("float UI format lacks color, blend, or sampling support");
    }
    Ok(())
}

fn sharing_family_indices(
    create_info: &vk::SwapchainCreateInfoKHR<'_>,
) -> Result<Vec<u32>, &'static str> {
    match create_info.image_sharing_mode {
        vk::SharingMode::EXCLUSIVE => Ok(Vec::new()),
        vk::SharingMode::CONCURRENT => {
            if create_info.queue_family_index_count == 0
                || create_info.p_queue_family_indices.is_null()
            {
                return Err("concurrent sharing has no queue-family list");
            }
            let indices = unsafe {
                std::slice::from_raw_parts(
                    create_info.p_queue_family_indices,
                    create_info.queue_family_index_count as usize,
                )
            };
            Ok(indices.to_vec())
        }
        _ => Err("swapchain sharing mode is unsupported"),
    }
}

unsafe fn surface_capabilities(
    context: &dispatch::DeviceSurfaceContext,
    surface: vk::SurfaceKHR,
) -> Option<vk::SurfaceCapabilitiesKHR> {
    let next = dispatch::next_instance_proc_addr(context.instance)?;
    let function = unsafe {
        next(
            context.instance,
            c"vkGetPhysicalDeviceSurfaceCapabilitiesKHR".as_ptr(),
        )
    }?;
    let query: vk::PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR =
        unsafe { std::mem::transmute_copy(&function) };
    let mut capabilities = vk::SurfaceCapabilitiesKHR::default();
    (unsafe { query(context.physical_device, surface, &mut capabilities) } == vk::Result::SUCCESS)
        .then_some(capabilities)
}

unsafe fn surface_present_families(
    context: &dispatch::DeviceSurfaceContext,
    surface: vk::SurfaceKHR,
    sharing_indices: &[u32],
) -> Result<HashSet<u32>, &'static str> {
    let Some(next) = dispatch::next_instance_proc_addr(context.instance) else {
        return Err("instance dispatch is unavailable for surface support");
    };
    let Some(function) = (unsafe {
        next(
            context.instance,
            c"vkGetPhysicalDeviceSurfaceSupportKHR".as_ptr(),
        )
    }) else {
        return Err("surface present-support query is unavailable");
    };
    let query: vk::PFN_vkGetPhysicalDeviceSurfaceSupportKHR =
        unsafe { std::mem::transmute_copy(&function) };
    let mut present_families = HashSet::new();
    for (family_index, family) in &context.queue_families {
        if family.requested_count == 0
            || !family
                .properties
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS)
        {
            continue;
        }
        if !sharing_indices.is_empty() && !sharing_indices.contains(family_index) {
            continue;
        }
        let mut supported = vk::FALSE;
        let result = unsafe {
            query(
                context.physical_device,
                *family_index,
                surface,
                &mut supported,
            )
        };
        if result != vk::Result::SUCCESS {
            return Err("surface present-support query failed");
        }
        if supported == vk::TRUE {
            present_families.insert(*family_index);
        }
    }
    Ok(present_families)
}

unsafe fn create_swapchain(
    device: vk::Device,
    create_info: *const vk::SwapchainCreateInfoKHR<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    swapchain: *mut vk::SwapchainKHR,
) -> vk::Result {
    if create_info.is_null() || swapchain.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let Some(function) =
        (unsafe { dispatch::next_device_function(device, c"vkCreateSwapchainKHR") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let create: vk::PFN_vkCreateSwapchainKHR = unsafe { std::mem::transmute_copy(&function) };
    let create_info_ref = unsafe { &*create_info };
    let queue_family_indices = sharing_family_indices(create_info_ref).unwrap_or_default();
    let mut eligibility = unsafe { evaluate_eligibility(device, create_info_ref) };
    let mut actual_image_usage = create_info_ref.image_usage;
    let needs_usage = eligibility.disable_reason.is_none()
        && !create_info_ref
            .image_usage
            .contains(eligibility.required_usage);

    let result = if needs_usage && create_info_ref.old_swapchain.is_null() {
        let mut extended = *create_info_ref;
        extended.image_usage |= eligibility.required_usage;
        let result = unsafe { create(device, &extended, allocator, swapchain) };
        if result == vk::Result::SUCCESS {
            actual_image_usage = extended.image_usage;
            result
        } else {
            unsafe { *swapchain = vk::SwapchainKHR::null() };
            eligibility.disable_reason = Some("required overlay image usage was rejected");
            eligibility.present_families.clear();
            unsafe { create(device, create_info, allocator, swapchain) }
        }
    } else {
        if needs_usage {
            eligibility.disable_reason =
                Some("old swapchain recreation cannot safely add required overlay image usage");
            eligibility.present_families.clear();
        }
        unsafe { create(device, create_info, allocator, swapchain) }
    };

    if result == vk::Result::SUCCESS && !unsafe { (*swapchain).is_null() } {
        let reason = eligibility.disable_reason;
        swapchains().insert(
            unsafe { *swapchain },
            SwapchainState {
                device,
                instance: dispatch::device_surface_context(device)
                    .map_or(vk::Instance::null(), |context| context.instance),
                format: create_info_ref.image_format,
                color_space: create_info_ref.image_color_space,
                output_color_mode: eligibility.output_color_mode,
                image_usage: actual_image_usage,
                extent: create_info_ref.image_extent,
                enabled: reason.is_none(),
                sharing_mode: create_info_ref.image_sharing_mode,
                queue_family_indices,
                present_families: eligibility.present_families,
                images: Vec::new(),
                acquired_images: HashSet::new(),
                presented_images: HashSet::new(),
                uncertain_images: HashSet::new(),
            },
        );
        if let Some(reason) = reason {
            report_disabled_once(reason);
        }
    }
    result
}

unsafe fn destroy_swapchain(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    allocator: *const vk::AllocationCallbacks<'_>,
) {
    dispatch::with_device_queue_serialization(device, || {
        crate::overlay_render::remove_swapchain(swapchain);
        if let Some(function) =
            unsafe { dispatch::next_device_function(device, c"vkDestroySwapchainKHR") }
        {
            let destroy: vk::PFN_vkDestroySwapchainKHR =
                unsafe { std::mem::transmute_copy(&function) };
            unsafe { destroy(device, swapchain, allocator) };
        }
        remove(swapchain);
    });
}

unsafe fn get_swapchain_images(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    image_count: *mut u32,
    images: *mut vk::Image,
) -> vk::Result {
    let Some(function) =
        (unsafe { dispatch::next_device_function(device, c"vkGetSwapchainImagesKHR") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let get_images: vk::PFN_vkGetSwapchainImagesKHR =
        unsafe { std::mem::transmute_copy(&function) };
    let result = unsafe { get_images(device, swapchain, image_count, images) };
    if result == vk::Result::SUCCESS && !image_count.is_null() && !images.is_null() {
        let image_count = unsafe { *image_count } as usize;
        let returned_images = unsafe { std::slice::from_raw_parts(images, image_count) }.to_vec();
        if let Some(state) = swapchains().get_mut(&swapchain) {
            state.images = returned_images;
            let images_len = state.images.len();
            state
                .acquired_images
                .retain(|index| (*index as usize) < images_len);
            state
                .presented_images
                .retain(|index| (*index as usize) < images_len);
            state
                .uncertain_images
                .retain(|index| (*index as usize) < images_len);
        }
    }
    result
}

unsafe fn acquire_next_image(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    timeout: u64,
    semaphore: vk::Semaphore,
    fence: vk::Fence,
    image_index: *mut u32,
) -> vk::Result {
    let Some(function) =
        (unsafe { dispatch::next_device_function(device, c"vkAcquireNextImageKHR") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let acquire: vk::PFN_vkAcquireNextImageKHR = unsafe { std::mem::transmute_copy(&function) };
    let result = unsafe { acquire(device, swapchain, timeout, semaphore, fence, image_index) };
    record_acquired_image(swapchain, result, image_index);
    result
}

unsafe fn acquire_next_image2(
    device: vk::Device,
    acquire_info: *const vk::AcquireNextImageInfoKHR<'_>,
    image_index: *mut u32,
) -> vk::Result {
    let Some(function) =
        (unsafe { dispatch::next_device_function(device, c"vkAcquireNextImage2KHR") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let acquire: vk::PFN_vkAcquireNextImage2KHR = unsafe { std::mem::transmute_copy(&function) };
    let result = unsafe { acquire(device, acquire_info, image_index) };
    if !acquire_info.is_null() {
        record_acquired_image(unsafe { (*acquire_info).swapchain }, result, image_index);
    }
    result
}

fn record_acquired_image(swapchain: vk::SwapchainKHR, result: vk::Result, image_index: *mut u32) {
    if !matches!(result, vk::Result::SUCCESS | vk::Result::SUBOPTIMAL_KHR) || image_index.is_null()
    {
        return;
    }
    let image_index = unsafe { *image_index };
    let mut recorded = false;
    if let Some(state) = swapchains().get_mut(&swapchain) {
        if state.images.is_empty() || (image_index as usize) < state.images.len() {
            state.acquired_images.insert(image_index);
            state.presented_images.remove(&image_index);
            state.uncertain_images.remove(&image_index);
            recorded = true;
        }
    }
    if recorded {
        crate::overlay_render::record_acquired(swapchain, image_index);
    }
}

unsafe fn queue_present(
    queue: vk::Queue,
    present_info: *const vk::PresentInfoKHR<'_>,
) -> vk::Result {
    let Some(metadata) = dispatch::queue_metadata(queue) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) =
        (unsafe { dispatch::next_device_function(metadata.device, c"vkQueuePresentKHR") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let present: vk::PFN_vkQueuePresentKHR = unsafe { std::mem::transmute_copy(&function) };
    dispatch::with_queue_serialization(queue, || unsafe {
        crate::overlay_render::queue_present(queue, present_info, present)
    })
}

pub(crate) fn record_present_results(
    present_info: *const vk::PresentInfoKHR<'_>,
    result: vk::Result,
) {
    if present_info.is_null() {
        return;
    }

    let info = unsafe { &*present_info };
    if info.swapchain_count == 0 || info.p_swapchains.is_null() || info.p_image_indices.is_null() {
        return;
    }

    let mut has_uncertain_result = false;
    {
        let mut registry = swapchains();
        for index in 0..info.swapchain_count as usize {
            let swapchain = unsafe { *info.p_swapchains.add(index) };
            let image_index = unsafe { *info.p_image_indices.add(index) };
            let image_result = if info.p_results.is_null() {
                result
            } else {
                unsafe { *info.p_results.add(index) }
            };
            let Some(state) = registry.get_mut(&swapchain) else {
                continue;
            };

            state.acquired_images.remove(&image_index);
            if matches!(
                image_result,
                vk::Result::SUCCESS | vk::Result::SUBOPTIMAL_KHR
            ) {
                state.uncertain_images.remove(&image_index);
                state.presented_images.insert(image_index);
            } else {
                state.presented_images.remove(&image_index);
                state.uncertain_images.insert(image_index);
                state.enabled = false;
                has_uncertain_result = true;
            }
        }
    }
    if has_uncertain_result {
        report_disabled_once("swapchain presentation left image synchronization uncertain");
    }
}
