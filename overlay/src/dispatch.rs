//! Loader ABI, instance dispatch, and Vulkan layer-chain traversal.
use ash::vk::{self, Handle};
use std::{
    collections::HashMap,
    ffi::{c_char, c_void, CStr},
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};

const NEGOTIATE_INTERFACE_STRUCT: i32 = 1;
const LAYER_INTERFACE_VERSION: u32 = 2;
const VK_LAYER_NAME: &[u8] = b"VK_LAYER_DSXINPUT_overlay\0";
const VK_LAYER_DESCRIPTION: &[u8] = b"dsxinput version overlay\0";

#[repr(C)]
pub(crate) struct NegotiateLayerInterface {
    s_type: i32,
    p_next: *mut c_void,
    loader_layer_interface_version: u32,
    pfn_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
    pfn_get_physical_device_proc_addr: Option<
        unsafe extern "system" fn(vk::Instance, *const std::ffi::c_char) -> vk::PFN_vkVoidFunction,
    >,
}

#[repr(C)]
struct LayerInstanceLink {
    p_next: *mut LayerInstanceLink,
    pfn_next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_next_get_physical_device_proc_addr:
        Option<unsafe extern "system" fn(vk::Instance, *const c_char) -> vk::PFN_vkVoidFunction>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LayerDeviceCallbacks {
    create: Option<unsafe extern "system" fn()>,
    destroy: Option<unsafe extern "system" fn()>,
}

#[repr(C)]
#[derive(Clone, Copy)]
union LayerInstanceUnion {
    p_layer_info: *mut LayerInstanceLink,
    pfn_set_instance_loader_data:
        Option<unsafe extern "system" fn(vk::Instance, *mut c_void) -> vk::Result>,
    layer_device: LayerDeviceCallbacks,
    loader_features: u32,
}

#[repr(C)]
struct LayerInstanceCreateInfo {
    s_type: vk::StructureType,
    p_next: *const c_void,
    function: i32,
    u: LayerInstanceUnion,
}

#[repr(C)]
struct LayerDeviceLink {
    p_next: *mut LayerDeviceLink,
    pfn_next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_next_get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

#[repr(C)]
#[derive(Clone, Copy)]
union LayerDeviceUnion {
    p_layer_info: *mut LayerDeviceLink,
    pfn_set_device_loader_data:
        Option<unsafe extern "system" fn(vk::Device, *mut c_void) -> vk::Result>,
}

#[repr(C)]
struct LayerDeviceCreateInfo {
    s_type: vk::StructureType,
    p_next: *const c_void,
    function: i32,
    u: LayerDeviceUnion,
}

#[repr(C)]
struct BaseInStructure {
    s_type: vk::StructureType,
    p_next: *const c_void,
}

type InstanceDispatch = HashMap<vk::Instance, vk::PFN_vkGetInstanceProcAddr>;
type PhysicalDeviceInstances = HashMap<vk::PhysicalDevice, vk::Instance>;
type DeviceDispatch = HashMap<vk::Device, vk::PFN_vkGetDeviceProcAddr>;
type DeviceStates = HashMap<vk::Device, DeviceState>;
type QueueDispatch = HashMap<vk::Queue, QueueState>;

pub(crate) struct SurfaceQueueFamily {
    pub(crate) requested_count: u32,
    pub(crate) properties: vk::QueueFamilyProperties,
}

pub(crate) struct DeviceSurfaceContext {
    pub(crate) instance: vk::Instance,
    pub(crate) physical_device: vk::PhysicalDevice,
    pub(crate) queue_families: HashMap<u32, SurfaceQueueFamily>,
}

#[derive(Clone)]
struct DeviceState {
    instance: vk::Instance,
    physical_device: vk::PhysicalDevice,
    queue_families: HashMap<u32, QueueFamilyState>,
}

#[derive(Clone, Copy)]
struct QueueFamilyState {
    requested_count: u32,
    properties: vk::QueueFamilyProperties,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueMetadata {
    pub(crate) device: vk::Device,
    pub(crate) instance: vk::Instance,
    pub(crate) physical_device: vk::PhysicalDevice,
    pub(crate) family_index: u32,
    pub(crate) queue_index: u32,
    pub(crate) flags: vk::DeviceQueueCreateFlags,
    pub(crate) family_flags: vk::QueueFlags,
}

impl QueueMetadata {
    fn new(
        device: vk::Device,
        family_index: u32,
        queue_index: u32,
        flags: vk::DeviceQueueCreateFlags,
    ) -> Self {
        Self {
            device,
            instance: vk::Instance::null(),
            physical_device: vk::PhysicalDevice::null(),
            family_index,
            queue_index,
            flags,
            family_flags: vk::QueueFlags::empty(),
        }
    }
}

#[derive(Clone)]
struct QueueState {
    metadata: QueueMetadata,
    serialization: Arc<Mutex<()>>,
}

impl QueueState {
    fn new(metadata: QueueMetadata) -> Self {
        Self {
            metadata,
            serialization: Arc::new(Mutex::new(())),
        }
    }
}

static INSTANCE_DISPATCH: OnceLock<Mutex<InstanceDispatch>> = OnceLock::new();
static PHYSICAL_DEVICE_INSTANCES: OnceLock<Mutex<PhysicalDeviceInstances>> = OnceLock::new();
static DEVICE_DISPATCH: OnceLock<Mutex<DeviceDispatch>> = OnceLock::new();
static DEVICE_STATES: OnceLock<Mutex<DeviceStates>> = OnceLock::new();
static QUEUE_DISPATCH: OnceLock<Mutex<QueueDispatch>> = OnceLock::new();

fn instance_dispatch() -> MutexGuard<'static, InstanceDispatch> {
    INSTANCE_DISPATCH
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn physical_device_instances() -> MutexGuard<'static, PhysicalDeviceInstances> {
    PHYSICAL_DEVICE_INSTANCES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn device_dispatch() -> MutexGuard<'static, DeviceDispatch> {
    DEVICE_DISPATCH
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn device_states() -> MutexGuard<'static, DeviceStates> {
    DEVICE_STATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn queue_dispatch() -> MutexGuard<'static, QueueDispatch> {
    QUEUE_DISPATCH
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[no_mangle]
pub unsafe extern "system" fn vkNegotiateLoaderLayerInterfaceVersion(
    interface: *mut NegotiateLayerInterface,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        negotiate_interface(interface)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkGetInstanceProcAddr(
    instance: vk::Instance,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let next = next_instance_proc_addr(instance);
        dispatch_instance_proc(instance, name, next)
    }))
    .unwrap_or(None)
}

#[no_mangle]
pub unsafe extern "system" fn vkGetDeviceProcAddr(
    device: vk::Device,
    name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        dispatch_device_proc(device, name, next_device_proc_addr(device))
    }))
    .unwrap_or(None)
}

#[no_mangle]
pub unsafe extern "system" fn vkCreateInstance(
    create_info: *const vk::InstanceCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    instance: *mut vk::Instance,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        create_instance(create_info, allocator, instance)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkDestroyInstance(
    instance: vk::Instance,
    allocator: *const vk::AllocationCallbacks<'_>,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        destroy_instance(instance, allocator)
    }));
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumerateInstanceLayerProperties(
    property_count: *mut u32,
    properties: *mut vk::LayerProperties,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_layer_properties(property_count, properties)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumerateDeviceLayerProperties(
    _physical_device: vk::PhysicalDevice,
    property_count: *mut u32,
    properties: *mut vk::LayerProperties,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_layer_properties(property_count, properties)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumerateInstanceExtensionProperties(
    layer_name: *const c_char,
    property_count: *mut u32,
    _properties: *mut vk::ExtensionProperties,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_layer_extensions(layer_name, property_count)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumerateDeviceExtensionProperties(
    physical_device: vk::PhysicalDevice,
    layer_name: *const c_char,
    property_count: *mut u32,
    properties: *mut vk::ExtensionProperties,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_device_extension_properties(
            physical_device,
            layer_name,
            property_count,
            properties,
        )
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumeratePhysicalDevices(
    instance: vk::Instance,
    physical_device_count: *mut u32,
    physical_devices: *mut vk::PhysicalDevice,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_physical_devices(instance, physical_device_count, physical_devices)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumeratePhysicalDeviceGroups(
    instance: vk::Instance,
    group_count: *mut u32,
    groups: *mut vk::PhysicalDeviceGroupProperties<'_>,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_physical_device_groups(
            instance,
            group_count,
            groups,
            c"vkEnumeratePhysicalDeviceGroups",
        )
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkEnumeratePhysicalDeviceGroupsKHR(
    instance: vk::Instance,
    group_count: *mut u32,
    groups: *mut vk::PhysicalDeviceGroupProperties<'_>,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        enumerate_physical_device_groups(
            instance,
            group_count,
            groups,
            c"vkEnumeratePhysicalDeviceGroupsKHR",
        )
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkCreateDevice(
    physical_device: vk::PhysicalDevice,
    create_info: *const vk::DeviceCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    device: *mut vk::Device,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        create_device(physical_device, create_info, allocator, device)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkDestroyDevice(
    device: vk::Device,
    allocator: *const vk::AllocationCallbacks<'_>,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        destroy_device(device, allocator)
    }));
}

#[no_mangle]
pub unsafe extern "system" fn vkGetDeviceQueue(
    device: vk::Device,
    queue_family_index: u32,
    queue_index: u32,
    queue: *mut vk::Queue,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        get_device_queue(
            device,
            queue_family_index,
            queue_index,
            vk::DeviceQueueCreateFlags::empty(),
            queue,
        )
    }));
}

#[no_mangle]
pub unsafe extern "system" fn vkGetDeviceQueue2(
    device: vk::Device,
    queue_info: *const vk::DeviceQueueInfo2<'_>,
    queue: *mut vk::Queue,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        get_device_queue2(device, queue_info, queue)
    }));
}

#[no_mangle]
pub unsafe extern "system" fn vkQueueSubmit(
    queue: vk::Queue,
    submit_count: u32,
    submits: *const vk::SubmitInfo<'_>,
    fence: vk::Fence,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        queue_submit(queue, submit_count, submits, fence)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkQueueSubmit2(
    queue: vk::Queue,
    submit_count: u32,
    submits: *const vk::SubmitInfo2<'_>,
    fence: vk::Fence,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        queue_submit2(queue, submit_count, submits, fence)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkQueueBindSparse(
    queue: vk::Queue,
    bind_info_count: u32,
    bind_infos: *const vk::BindSparseInfo<'_>,
    fence: vk::Fence,
) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        queue_bind_sparse(queue, bind_info_count, bind_infos, fence)
    }))
    .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkQueueWaitIdle(queue: vk::Queue) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { queue_wait_idle(queue) }))
        .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

#[no_mangle]
pub unsafe extern "system" fn vkDeviceWaitIdle(device: vk::Device) -> vk::Result {
    std::panic::catch_unwind(AssertUnwindSafe(|| unsafe { device_wait_idle(device) }))
        .unwrap_or(vk::Result::ERROR_INITIALIZATION_FAILED)
}

unsafe fn negotiate_interface(interface: *mut NegotiateLayerInterface) -> vk::Result {
    if interface.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let interface = unsafe { &mut *interface };
    if interface.s_type != NEGOTIATE_INTERFACE_STRUCT
        || interface.loader_layer_interface_version == 0
    {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    interface.loader_layer_interface_version = interface
        .loader_layer_interface_version
        .min(LAYER_INTERFACE_VERSION);
    interface.pfn_get_instance_proc_addr = vkGetInstanceProcAddr;
    interface.pfn_get_device_proc_addr = vkGetDeviceProcAddr;
    interface.pfn_get_physical_device_proc_addr = None;
    vk::Result::SUCCESS
}

unsafe fn dispatch_instance_proc(
    instance: vk::Instance,
    name: *const c_char,
    next: Option<vk::PFN_vkGetInstanceProcAddr>,
) -> vk::PFN_vkVoidFunction {
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) };
    let swapchain_function = unsafe { crate::swapchain::instance_proc(instance, name, next) };
    if swapchain_function.is_some() {
        return swapchain_function;
    }
    if name.to_bytes() == b"vkGetInstanceProcAddr" {
        return erase_function(vkGetInstanceProcAddr as vk::PFN_vkGetInstanceProcAddr);
    }
    if name.to_bytes() == b"vkGetDeviceProcAddr" {
        return erase_function(vkGetDeviceProcAddr as vk::PFN_vkGetDeviceProcAddr);
    }
    if name.to_bytes() == b"vkCreateInstance" {
        return erase_function(vkCreateInstance as vk::PFN_vkCreateInstance);
    }
    if name.to_bytes() == b"vkDestroyInstance" {
        return erase_function(vkDestroyInstance as vk::PFN_vkDestroyInstance);
    }
    if name.to_bytes() == b"vkEnumerateInstanceLayerProperties" {
        return erase_function(
            vkEnumerateInstanceLayerProperties as vk::PFN_vkEnumerateInstanceLayerProperties,
        );
    }
    if name.to_bytes() == b"vkEnumerateInstanceExtensionProperties" {
        return erase_function(
            vkEnumerateInstanceExtensionProperties
                as vk::PFN_vkEnumerateInstanceExtensionProperties,
        );
    }
    if name.to_bytes() == b"vkEnumerateDeviceLayerProperties" {
        return erase_function(
            vkEnumerateDeviceLayerProperties as vk::PFN_vkEnumerateDeviceLayerProperties,
        );
    }
    if name.to_bytes() == b"vkEnumerateDeviceExtensionProperties" {
        return erase_function(
            vkEnumerateDeviceExtensionProperties as vk::PFN_vkEnumerateDeviceExtensionProperties,
        );
    }
    if name.to_bytes() == b"vkEnumeratePhysicalDevices" {
        return erase_function(vkEnumeratePhysicalDevices as vk::PFN_vkEnumeratePhysicalDevices);
    }
    if name.to_bytes() == b"vkEnumeratePhysicalDeviceGroups"
        || name.to_bytes() == b"vkEnumeratePhysicalDeviceGroupsKHR"
    {
        let Some(next) = next else {
            return None;
        };
        if unsafe { next(instance, name.as_ptr()) }.is_none() {
            return None;
        }
        let function = if name.to_bytes() == b"vkEnumeratePhysicalDeviceGroupsKHR" {
            vkEnumeratePhysicalDeviceGroupsKHR as vk::PFN_vkEnumeratePhysicalDeviceGroups
        } else {
            vkEnumeratePhysicalDeviceGroups as vk::PFN_vkEnumeratePhysicalDeviceGroups
        };
        return erase_function(function);
    }
    if name.to_bytes() == b"vkCreateDevice" {
        return erase_function(vkCreateDevice as vk::PFN_vkCreateDevice);
    }
    next.and_then(|next| unsafe { next(instance, name.as_ptr()) })
}

unsafe fn dispatch_device_proc(
    device: vk::Device,
    name: *const c_char,
    next: Option<vk::PFN_vkGetDeviceProcAddr>,
) -> vk::PFN_vkVoidFunction {
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) };
    let swapchain_function = unsafe { crate::swapchain::device_proc(device, name, next) };
    if swapchain_function.is_some() {
        return swapchain_function;
    }
    if name.to_bytes() == b"vkGetDeviceProcAddr" {
        return erase_function(vkGetDeviceProcAddr as vk::PFN_vkGetDeviceProcAddr);
    }
    if name.to_bytes() == b"vkDestroyDevice" {
        return erase_function(vkDestroyDevice as vk::PFN_vkDestroyDevice);
    }
    if name.to_bytes() == b"vkDeviceWaitIdle" {
        return erase_function(vkDeviceWaitIdle as vk::PFN_vkDeviceWaitIdle);
    }
    if name.to_bytes() == b"vkGetDeviceQueue" {
        return erase_function(vkGetDeviceQueue as vk::PFN_vkGetDeviceQueue);
    }
    if name.to_bytes() == b"vkGetDeviceQueue2" {
        let Some(next) = next else {
            return None;
        };
        if unsafe { next(device, name.as_ptr()) }.is_none() {
            return None;
        }
        return erase_function(vkGetDeviceQueue2 as vk::PFN_vkGetDeviceQueue2);
    }
    if name.to_bytes() == b"vkQueueSubmit" {
        return erase_function(vkQueueSubmit as vk::PFN_vkQueueSubmit);
    }
    if name.to_bytes() == b"vkQueueSubmit2" || name.to_bytes() == b"vkQueueSubmit2KHR" {
        let Some(next) = next else {
            return None;
        };
        if unsafe { next(device, name.as_ptr()) }.is_none() {
            return None;
        }
        return erase_function(vkQueueSubmit2 as vk::PFN_vkQueueSubmit2);
    }
    if name.to_bytes() == b"vkQueueBindSparse" {
        return erase_function(vkQueueBindSparse as vk::PFN_vkQueueBindSparse);
    }
    if name.to_bytes() == b"vkQueueWaitIdle" {
        return erase_function(vkQueueWaitIdle as vk::PFN_vkQueueWaitIdle);
    }
    next.and_then(|next| unsafe { next(device, name.as_ptr()) })
}

fn erase_function<T: Copy>(function: T) -> vk::PFN_vkVoidFunction {
    assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<unsafe extern "system" fn()>()
    );
    Some(unsafe { std::mem::transmute_copy(&function) })
}

unsafe fn next_create_instance(
    next_gipa: vk::PFN_vkGetInstanceProcAddr,
) -> Option<vk::PFN_vkCreateInstance> {
    let name = c"vkCreateInstance";
    let function = unsafe { next_gipa(vk::Instance::null(), name.as_ptr()) }?;
    Some(unsafe { std::mem::transmute_copy(&function) })
}

unsafe fn find_link_info(
    create_info: *const vk::InstanceCreateInfo<'_>,
) -> Option<*mut LayerInstanceCreateInfo> {
    if create_info.is_null() {
        return None;
    }
    let mut node = unsafe { (*create_info).p_next };
    while !node.is_null() {
        let base = unsafe { &*(node as *const BaseInStructure) };
        let next = base.p_next;
        if base.s_type == vk::StructureType::LOADER_INSTANCE_CREATE_INFO {
            let loader_info = node as *mut LayerInstanceCreateInfo;
            let function = unsafe { (*loader_info).function };
            if function == 0 {
                return Some(loader_info);
            }
        }
        node = next;
    }
    None
}

unsafe fn find_device_link_info(
    create_info: *const vk::DeviceCreateInfo<'_>,
) -> Option<*mut LayerDeviceCreateInfo> {
    if create_info.is_null() {
        return None;
    }
    let mut node = unsafe { (*create_info).p_next };
    while !node.is_null() {
        let base = unsafe { &*(node as *const BaseInStructure) };
        let next = base.p_next;
        if base.s_type == vk::StructureType::LOADER_DEVICE_CREATE_INFO {
            let loader_info = node as *mut LayerDeviceCreateInfo;
            let function = unsafe { (*loader_info).function };
            if function == 0 {
                return Some(loader_info);
            }
        }
        node = next;
    }
    None
}

unsafe fn create_instance(
    create_info: *const vk::InstanceCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    instance: *mut vk::Instance,
) -> vk::Result {
    if create_info.is_null() || instance.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let Some(loader_info) = (unsafe { find_link_info(create_info) }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let link = unsafe { (*loader_info).u.p_layer_info };
    if link.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let next_gipa = unsafe { (*link).pfn_next_get_instance_proc_addr };
    let Some(create_next) = (unsafe { next_create_instance(next_gipa) }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };

    // The loader gives each layer its own link node. Advance before calling down.
    unsafe { (*loader_info).u.p_layer_info = (*link).p_next };
    let result = unsafe { create_next(create_info, allocator, instance) };
    if result == vk::Result::SUCCESS && !unsafe { (*instance).is_null() } {
        instance_dispatch().insert(unsafe { *instance }, next_gipa);
    }
    result
}

unsafe fn destroy_instance(instance: vk::Instance, allocator: *const vk::AllocationCallbacks<'_>) {
    crate::overlay_render::remove_instance(instance);
    let next = instance_dispatch().get(&instance).copied();
    if let Some(next) = next {
        let name = c"vkDestroyInstance";
        if let Some(function) = unsafe { next(instance, name.as_ptr()) } {
            let destroy: vk::PFN_vkDestroyInstance = unsafe { std::mem::transmute_copy(&function) };
            unsafe { destroy(instance, allocator) };
        }
    }
    crate::swapchain::remove_instance(instance);
    instance_dispatch().remove(&instance);
    physical_device_instances().retain(|_, physical_instance| *physical_instance != instance);
}

pub(crate) fn next_instance_proc_addr(
    instance: vk::Instance,
) -> Option<vk::PFN_vkGetInstanceProcAddr> {
    let dispatch = instance_dispatch();
    dispatch.get(&instance).copied().or_else(|| {
        if instance.is_null() {
            dispatch.values().next().copied()
        } else {
            None
        }
    })
}

pub(crate) fn next_device_proc_addr(device: vk::Device) -> Option<vk::PFN_vkGetDeviceProcAddr> {
    device_dispatch().get(&device).copied()
}

pub(crate) unsafe fn next_device_function(
    device: vk::Device,
    name: &CStr,
) -> vk::PFN_vkVoidFunction {
    let Some(next) = next_device_proc_addr(device) else {
        return None;
    };
    unsafe { next(device, name.as_ptr()) }
}

pub(crate) fn queue_metadata(queue: vk::Queue) -> Option<QueueMetadata> {
    queue_dispatch().get(&queue).map(|state| state.metadata)
}

pub(crate) fn device_surface_context(device: vk::Device) -> Option<DeviceSurfaceContext> {
    let state = device_states().get(&device).cloned()?;
    Some(DeviceSurfaceContext {
        instance: state.instance,
        physical_device: state.physical_device,
        queue_families: state
            .queue_families
            .into_iter()
            .map(|(index, family)| {
                (
                    index,
                    SurfaceQueueFamily {
                        requested_count: family.requested_count,
                        properties: family.properties,
                    },
                )
            })
            .collect(),
    })
}

pub(crate) fn with_queue_serialization<T>(queue: vk::Queue, call: impl FnOnce() -> T) -> T {
    let serialization = queue_dispatch()
        .get(&queue)
        .map(|state| Arc::clone(&state.serialization));
    if let Some(serialization) = serialization {
        let _guard = serialization
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        call()
    } else {
        call()
    }
}

fn device_queue_locks(device: vk::Device) -> Vec<(u64, Arc<Mutex<()>>)> {
    let mut locks = queue_dispatch()
        .iter()
        .filter(|(_, state)| state.metadata.device == device)
        .map(|(queue, state)| (queue.as_raw(), Arc::clone(&state.serialization)))
        .collect::<Vec<_>>();
    locks.sort_by_key(|(queue, _)| *queue);
    locks
}

pub(crate) fn with_device_queue_serialization<T>(
    device: vk::Device,
    call: impl FnOnce() -> T,
) -> T {
    let locks = device_queue_locks(device);
    let _guards = locks
        .iter()
        .map(|(_, lock)| {
            lock.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        })
        .collect::<Vec<_>>();
    call()
}

fn record_queue(
    queue: vk::Queue,
    device: vk::Device,
    family_index: u32,
    queue_index: u32,
    flags: vk::DeviceQueueCreateFlags,
) {
    if queue.is_null() {
        return;
    }
    let metadata = if let Some(device_state) = device_states().get(&device).cloned() {
        let Some(family) = device_state.queue_families.get(&family_index) else {
            return;
        };
        if queue_index >= family.requested_count {
            return;
        }
        QueueMetadata {
            device,
            instance: device_state.instance,
            physical_device: device_state.physical_device,
            family_index,
            queue_index,
            flags,
            family_flags: family.properties.queue_flags,
        }
    } else {
        QueueMetadata::new(device, family_index, queue_index, flags)
    };
    let mut queues = queue_dispatch();
    queues
        .entry(queue)
        .and_modify(|state| state.metadata = metadata)
        .or_insert_with(|| QueueState::new(metadata));
}

unsafe fn get_device_queue(
    device: vk::Device,
    family_index: u32,
    queue_index: u32,
    flags: vk::DeviceQueueCreateFlags,
    queue: *mut vk::Queue,
) {
    if queue.is_null() {
        return;
    }
    let Some(function) = (unsafe { next_device_function(device, c"vkGetDeviceQueue") }) else {
        return;
    };
    let get_queue: vk::PFN_vkGetDeviceQueue = unsafe { std::mem::transmute_copy(&function) };
    unsafe { get_queue(device, family_index, queue_index, queue) };
    record_queue(unsafe { *queue }, device, family_index, queue_index, flags);
}

unsafe fn get_device_queue2(
    device: vk::Device,
    queue_info: *const vk::DeviceQueueInfo2<'_>,
    queue: *mut vk::Queue,
) {
    if queue.is_null() {
        return;
    }
    let Some(function) = (unsafe { next_device_function(device, c"vkGetDeviceQueue2") }) else {
        return;
    };
    let get_queue: vk::PFN_vkGetDeviceQueue2 = unsafe { std::mem::transmute_copy(&function) };
    unsafe { get_queue(device, queue_info, queue) };
    if !queue_info.is_null() {
        let info = unsafe { &*queue_info };
        record_queue(
            unsafe { *queue },
            device,
            info.queue_family_index,
            info.queue_index,
            info.flags,
        );
    }
}

unsafe fn queue_submit(
    queue: vk::Queue,
    submit_count: u32,
    submits: *const vk::SubmitInfo<'_>,
    fence: vk::Fence,
) -> vk::Result {
    let Some(metadata) = queue_metadata(queue) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) = (unsafe { next_device_function(metadata.device, c"vkQueueSubmit") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let submit: vk::PFN_vkQueueSubmit = unsafe { std::mem::transmute_copy(&function) };
    with_queue_serialization(queue, || unsafe {
        submit(queue, submit_count, submits, fence)
    })
}

unsafe fn queue_submit2(
    queue: vk::Queue,
    submit_count: u32,
    submits: *const vk::SubmitInfo2<'_>,
    fence: vk::Fence,
) -> vk::Result {
    let Some(metadata) = queue_metadata(queue) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let function = unsafe { next_device_function(metadata.device, c"vkQueueSubmit2") }
        .or_else(|| unsafe { next_device_function(metadata.device, c"vkQueueSubmit2KHR") });
    let Some(function) = function else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let submit: vk::PFN_vkQueueSubmit2 = unsafe { std::mem::transmute_copy(&function) };
    with_queue_serialization(queue, || unsafe {
        submit(queue, submit_count, submits, fence)
    })
}

unsafe fn queue_bind_sparse(
    queue: vk::Queue,
    bind_info_count: u32,
    bind_infos: *const vk::BindSparseInfo<'_>,
    fence: vk::Fence,
) -> vk::Result {
    let Some(metadata) = queue_metadata(queue) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) = (unsafe { next_device_function(metadata.device, c"vkQueueBindSparse") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let bind: vk::PFN_vkQueueBindSparse = unsafe { std::mem::transmute_copy(&function) };
    with_queue_serialization(queue, || unsafe {
        bind(queue, bind_info_count, bind_infos, fence)
    })
}

unsafe fn queue_wait_idle(queue: vk::Queue) -> vk::Result {
    let Some(metadata) = queue_metadata(queue) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) = (unsafe { next_device_function(metadata.device, c"vkQueueWaitIdle") })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let wait_idle: vk::PFN_vkQueueWaitIdle = unsafe { std::mem::transmute_copy(&function) };
    with_queue_serialization(queue, || unsafe { wait_idle(queue) })
}

unsafe fn device_wait_idle(device: vk::Device) -> vk::Result {
    let Some(function) = (unsafe { next_device_function(device, c"vkDeviceWaitIdle") }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let wait_idle: vk::PFN_vkDeviceWaitIdle = unsafe { std::mem::transmute_copy(&function) };
    with_device_queue_serialization(device, || unsafe { wait_idle(device) })
}

unsafe fn enumerate_physical_devices(
    instance: vk::Instance,
    physical_device_count: *mut u32,
    physical_devices: *mut vk::PhysicalDevice,
) -> vk::Result {
    if physical_device_count.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let Some(next) = next_instance_proc_addr(instance) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) = (unsafe { next(instance, c"vkEnumeratePhysicalDevices".as_ptr()) }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let enumerate: vk::PFN_vkEnumeratePhysicalDevices =
        unsafe { std::mem::transmute_copy(&function) };
    let result = unsafe { enumerate(instance, physical_device_count, physical_devices) };
    if (result == vk::Result::SUCCESS || result == vk::Result::INCOMPLETE)
        && !physical_devices.is_null()
    {
        let count = unsafe { *physical_device_count };
        let mut associations = physical_device_instances();
        for index in 0..count as usize {
            let physical_device = unsafe { *physical_devices.add(index) };
            if !physical_device.is_null() {
                associations.insert(physical_device, instance);
            }
        }
    }
    result
}

unsafe fn enumerate_physical_device_groups(
    instance: vk::Instance,
    group_count: *mut u32,
    groups: *mut vk::PhysicalDeviceGroupProperties<'_>,
    function_name: &CStr,
) -> vk::Result {
    if group_count.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let Some(next) = next_instance_proc_addr(instance) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) = (unsafe { next(instance, function_name.as_ptr()) }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let enumerate: vk::PFN_vkEnumeratePhysicalDeviceGroups =
        unsafe { std::mem::transmute_copy(&function) };
    let result = unsafe { enumerate(instance, group_count, groups) };
    if (result == vk::Result::SUCCESS || result == vk::Result::INCOMPLETE) && !groups.is_null() {
        let mut associations = physical_device_instances();
        for group_index in 0..unsafe { *group_count } as usize {
            let group = unsafe { &*groups.add(group_index) };
            let device_count = group
                .physical_device_count
                .min(vk::MAX_DEVICE_GROUP_SIZE as u32) as usize;
            for physical_device in group.physical_devices.iter().take(device_count) {
                if !physical_device.is_null() {
                    associations.insert(*physical_device, instance);
                }
            }
        }
    }
    result
}

unsafe fn requested_queue_family_counts(
    create_info: *const vk::DeviceCreateInfo<'_>,
) -> HashMap<u32, u32> {
    if create_info.is_null() {
        return HashMap::new();
    }
    let create_info = unsafe { &*create_info };
    if create_info.queue_create_info_count == 0 || create_info.p_queue_create_infos.is_null() {
        return HashMap::new();
    }
    unsafe {
        std::slice::from_raw_parts(
            create_info.p_queue_create_infos,
            create_info.queue_create_info_count as usize,
        )
    }
    .iter()
    .map(|queue_info| (queue_info.queue_family_index, queue_info.queue_count))
    .collect()
}

unsafe fn query_queue_family_properties(
    instance: vk::Instance,
    physical_device: vk::PhysicalDevice,
    next_gipa: vk::PFN_vkGetInstanceProcAddr,
) -> HashMap<u32, vk::QueueFamilyProperties> {
    let Some(function) = (unsafe {
        next_gipa(
            instance,
            c"vkGetPhysicalDeviceQueueFamilyProperties".as_ptr(),
        )
    }) else {
        return HashMap::new();
    };
    let get_properties: vk::PFN_vkGetPhysicalDeviceQueueFamilyProperties =
        unsafe { std::mem::transmute_copy(&function) };
    let mut count = 0;
    unsafe { get_properties(physical_device, &mut count, std::ptr::null_mut()) };
    if count == 0 {
        return HashMap::new();
    }
    let mut properties = vec![vk::QueueFamilyProperties::default(); count as usize];
    unsafe { get_properties(physical_device, &mut count, properties.as_mut_ptr()) };
    properties
        .into_iter()
        .take(count as usize)
        .enumerate()
        .map(|(index, properties)| (index as u32, properties))
        .collect()
}

unsafe fn create_device(
    physical_device: vk::PhysicalDevice,
    create_info: *const vk::DeviceCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    device: *mut vk::Device,
) -> vk::Result {
    if create_info.is_null() || device.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let Some(instance) = physical_device_instances().get(&physical_device).copied() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(loader_info) = (unsafe { find_device_link_info(create_info) }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let link = unsafe { (*loader_info).u.p_layer_info };
    if link.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let next_gipa = unsafe { (*link).pfn_next_get_instance_proc_addr };
    let next_gdpa = unsafe { (*link).pfn_next_get_device_proc_addr };
    let Some(function) = (unsafe { next_gipa(instance, c"vkCreateDevice".as_ptr()) }) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let create_next: vk::PFN_vkCreateDevice = unsafe { std::mem::transmute_copy(&function) };
    let queue_family_counts = unsafe { requested_queue_family_counts(create_info) };

    // Pass the next layer's link node down before forwarding device creation.
    unsafe { (*loader_info).u.p_layer_info = (*link).p_next };
    let result = unsafe { create_next(physical_device, create_info, allocator, device) };
    if result == vk::Result::SUCCESS && !unsafe { (*device).is_null() } {
        let family_properties =
            unsafe { query_queue_family_properties(instance, physical_device, next_gipa) };
        let queue_families = queue_family_counts
            .into_iter()
            .map(|(family_index, requested_count)| {
                (
                    family_index,
                    QueueFamilyState {
                        requested_count,
                        properties: family_properties
                            .get(&family_index)
                            .copied()
                            .unwrap_or_default(),
                    },
                )
            })
            .collect();
        device_dispatch().insert(unsafe { *device }, next_gdpa);
        device_states().insert(
            unsafe { *device },
            DeviceState {
                instance,
                physical_device,
                queue_families,
            },
        );
    }
    result
}

unsafe fn destroy_device(device: vk::Device, allocator: *const vk::AllocationCallbacks<'_>) {
    let queue_locks = device_queue_locks(device);
    let _queue_guards = queue_locks
        .iter()
        .map(|(_, lock)| {
            lock.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        })
        .collect::<Vec<_>>();

    crate::overlay_render::remove_device(device);
    if let Some(next) = next_device_proc_addr(device) {
        if let Some(function) = unsafe { next(device, c"vkDestroyDevice".as_ptr()) } {
            let destroy: vk::PFN_vkDestroyDevice = unsafe { std::mem::transmute_copy(&function) };
            unsafe { destroy(device, allocator) };
        }
    }
    crate::swapchain::remove_device(device);
    device_dispatch().remove(&device);
    device_states().remove(&device);
    queue_dispatch().retain(|_, state| state.metadata.device != device);
}

unsafe fn enumerate_layer_properties(
    property_count: *mut u32,
    properties: *mut vk::LayerProperties,
) -> vk::Result {
    if property_count.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    if properties.is_null() {
        unsafe { *property_count = 1 };
        return vk::Result::SUCCESS;
    }
    if unsafe { *property_count } == 0 {
        return vk::Result::INCOMPLETE;
    }
    let mut property = vk::LayerProperties::default();
    for (destination, source) in property.layer_name.iter_mut().zip(VK_LAYER_NAME) {
        *destination = *source as c_char;
    }
    for (destination, source) in property.description.iter_mut().zip(VK_LAYER_DESCRIPTION) {
        *destination = *source as c_char;
    }
    property.spec_version = vk::make_api_version(0, 1, 0, 0);
    property.implementation_version = 1;
    unsafe {
        *properties = property;
        *property_count = 1;
    }
    vk::Result::SUCCESS
}

unsafe fn enumerate_layer_extensions(
    layer_name: *const c_char,
    property_count: *mut u32,
) -> vk::Result {
    if property_count.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    if layer_name.is_null()
        || unsafe { CStr::from_ptr(layer_name) }.to_bytes_with_nul() != VK_LAYER_NAME
    {
        return vk::Result::ERROR_LAYER_NOT_PRESENT;
    }
    unsafe { *property_count = 0 };
    vk::Result::SUCCESS
}

unsafe fn enumerate_device_extension_properties(
    physical_device: vk::PhysicalDevice,
    layer_name: *const c_char,
    property_count: *mut u32,
    properties: *mut vk::ExtensionProperties,
) -> vk::Result {
    if !layer_name.is_null() {
        return unsafe { enumerate_layer_extensions(layer_name, property_count) };
    }
    if property_count.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    let Some(instance) = physical_device_instances().get(&physical_device).copied() else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(next) = next_instance_proc_addr(instance) else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let Some(function) =
        (unsafe { next(instance, c"vkEnumerateDeviceExtensionProperties".as_ptr()) })
    else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    let enumerate: vk::PFN_vkEnumerateDeviceExtensionProperties =
        unsafe { std::mem::transmute_copy(&function) };
    unsafe { enumerate(physical_device, layer_name, property_count, properties) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swapchain;
    use ash::vk::Handle;
    use std::{
        ffi::{c_char, CStr},
        sync::atomic::{AtomicUsize, Ordering},
    };

    static FORWARDED_INSTANCE: AtomicUsize = AtomicUsize::new(0);
    static FORWARDED_DEVICE: AtomicUsize = AtomicUsize::new(0);
    static FORWARDED_QUEUE: AtomicUsize = AtomicUsize::new(0);
    static ACTIVE_QUEUE_OPERATIONS: AtomicUsize = AtomicUsize::new(0);
    static MAX_CONCURRENT_QUEUE_OPERATIONS: AtomicUsize = AtomicUsize::new(0);
    static QUEUE_SUBMIT2_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DESTROYED_DEVICES: AtomicUsize = AtomicUsize::new(0);
    static DEVICE_EXTENSION_ENUMERATIONS: AtomicUsize = AtomicUsize::new(0);
    pub(crate) static DISPATCH_TEST_LOCK: Mutex<()> = Mutex::new(());
    static SWAPCHAIN_CREATE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static SWAPCHAIN_CREATE_USAGES: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
    static SWAPCHAIN_CREATE_LAST_USAGE: AtomicUsize = AtomicUsize::new(0);
    static SWAPCHAIN_CREATE_PNEXT: AtomicUsize = AtomicUsize::new(0);
    static SWAPCHAIN_FAIL_ADDED_USAGE: AtomicUsize = AtomicUsize::new(0);
    static SWAPCHAIN_DESTROY_CALLS: AtomicUsize = AtomicUsize::new(0);
    static SWAPCHAIN_IMAGE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static ACQUIRE_RESULT: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(vk::Result::SUCCESS.as_raw());
    static ACQUIRE2_RESULT: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(vk::Result::SUCCESS.as_raw());
    static ACQUIRE2_PNEXT: AtomicUsize = AtomicUsize::new(0);
    static PRESENT_RESULT: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(vk::Result::SUCCESS.as_raw());
    static PRESENT_FIRST_RESULT: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(vk::Result::SUCCESS.as_raw());
    static PRESENT_SECOND_RESULT: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(vk::Result::SUBOPTIMAL_KHR.as_raw());
    static PRESENT_PNEXT: AtomicUsize = AtomicUsize::new(0);
    static PRESENT_RESULTS: AtomicUsize = AtomicUsize::new(0);
    static SURFACE_COLOR_ATTACHMENT_SUPPORTED: AtomicUsize = AtomicUsize::new(1);
    static SURFACE_TRANSFER_SRC_SUPPORTED: AtomicUsize = AtomicUsize::new(1);
    static HDR_SCENE_IMAGE_SUPPORTED: AtomicUsize = AtomicUsize::new(1);
    static HDR_UI_IMAGE_SUPPORTED: AtomicUsize = AtomicUsize::new(1);
    static HDR_UI_BLEND_SUPPORTED: AtomicUsize = AtomicUsize::new(1);

    unsafe extern "system" fn next_gipa(
        _instance: vk::Instance,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        if name.is_null() {
            return None;
        }
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkCreateInstance" {
            return Some(unsafe {
                std::mem::transmute::<vk::PFN_vkCreateInstance, unsafe extern "system" fn()>(
                    next_create_instance,
                )
            });
        }
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkCreateDevice" {
            return Some(unsafe {
                std::mem::transmute::<vk::PFN_vkCreateDevice, unsafe extern "system" fn()>(
                    next_create_device,
                )
            });
        }
        if matches!(
            unsafe { CStr::from_ptr(name) }.to_bytes(),
            b"vkEnumeratePhysicalDeviceGroups" | b"vkEnumeratePhysicalDeviceGroupsKHR"
        ) {
            return Some(unsafe {
                std::mem::transmute::<
                    vk::PFN_vkEnumeratePhysicalDeviceGroups,
                    unsafe extern "system" fn(),
                >(next_enumerate_physical_device_groups)
            });
        }
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkGetPhysicalDeviceQueueFamilyProperties"
        {
            return Some(unsafe {
                std::mem::transmute::<
                    vk::PFN_vkGetPhysicalDeviceQueueFamilyProperties,
                    unsafe extern "system" fn(),
                >(next_queue_family_properties)
            });
        }
        Some(next_unknown_proc)
    }

    unsafe extern "system" fn recording_gipa(
        instance: vk::Instance,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        FORWARDED_INSTANCE.store(instance.as_raw() as usize, Ordering::Relaxed);
        if !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkCreateInstance" {
            return Some(unsafe {
                std::mem::transmute::<vk::PFN_vkCreateInstance, unsafe extern "system" fn()>(
                    next_create_instance,
                )
            });
        }
        Some(next_unknown_proc)
    }

    unsafe extern "system" fn next_create_instance(
        create_info: *const vk::InstanceCreateInfo<'_>,
        _allocator: *const vk::AllocationCallbacks<'_>,
        instance: *mut vk::Instance,
    ) -> vk::Result {
        if !create_info.is_null() && !instance.is_null() {
            unsafe { *instance = vk::Instance::from_raw(0x1234) };
        }
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_unknown_proc() {}

    unsafe extern "system" fn next_create_device(
        _physical_device: vk::PhysicalDevice,
        _create_info: *const vk::DeviceCreateInfo<'_>,
        _allocator: *const vk::AllocationCallbacks<'_>,
        device: *mut vk::Device,
    ) -> vk::Result {
        if !device.is_null() {
            unsafe { *device = vk::Device::from_raw(0x7788) };
        }
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_queue_family_properties(
        _physical_device: vk::PhysicalDevice,
        property_count: *mut u32,
        properties: *mut vk::QueueFamilyProperties,
    ) {
        if property_count.is_null() {
            return;
        }
        if properties.is_null() {
            unsafe { *property_count = 2 };
            return;
        }
        let count = unsafe { (*property_count).min(2) } as usize;
        if count > 0 {
            unsafe {
                *properties = vk::QueueFamilyProperties::default()
                    .queue_flags(vk::QueueFlags::TRANSFER)
                    .queue_count(1)
            };
        }
        if count > 1 {
            unsafe {
                *properties.add(1) = vk::QueueFamilyProperties::default()
                    .queue_flags(
                        vk::QueueFlags::GRAPHICS
                            | vk::QueueFlags::COMPUTE
                            | vk::QueueFlags::TRANSFER,
                    )
                    .queue_count(4)
            };
        }
        unsafe { *property_count = count as u32 };
    }

    unsafe extern "system" fn next_enumerate_physical_device_groups(
        _instance: vk::Instance,
        group_count: *mut u32,
        groups: *mut vk::PhysicalDeviceGroupProperties<'_>,
    ) -> vk::Result {
        if group_count.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        if groups.is_null() {
            unsafe { *group_count = 1 };
            return vk::Result::SUCCESS;
        }
        if unsafe { *group_count } == 0 {
            return vk::Result::INCOMPLETE;
        }
        let mut group = vk::PhysicalDeviceGroupProperties::default();
        group.physical_device_count = 2;
        group.physical_devices[0] = vk::PhysicalDevice::from_raw(0x5566);
        group.physical_devices[1] = vk::PhysicalDevice::from_raw(0x5567);
        unsafe {
            *groups = group;
            *group_count = 1;
        }
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_gdpa(
        device: vk::Device,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        FORWARDED_DEVICE.store(device.as_raw() as usize, Ordering::Relaxed);
        if !name.is_null() {
            match unsafe { CStr::from_ptr(name) }.to_bytes() {
                b"vkGetDeviceQueue" => {
                    return Some(unsafe {
                        std::mem::transmute::<vk::PFN_vkGetDeviceQueue, unsafe extern "system" fn()>(
                            next_get_device_queue,
                        )
                    });
                }
                b"vkGetDeviceQueue2" => {
                    return Some(unsafe {
                        std::mem::transmute::<vk::PFN_vkGetDeviceQueue2, unsafe extern "system" fn()>(
                            next_get_device_queue2,
                        )
                    });
                }
                b"vkQueueSubmit" => {
                    return Some(unsafe {
                        std::mem::transmute::<vk::PFN_vkQueueSubmit, unsafe extern "system" fn()>(
                            next_queue_submit,
                        )
                    });
                }
                b"vkQueueSubmit2" => return None,
                b"vkQueueSubmit2KHR" => {
                    return Some(unsafe {
                        std::mem::transmute::<vk::PFN_vkQueueSubmit2, unsafe extern "system" fn()>(
                            next_queue_submit2,
                        )
                    });
                }
                b"vkDeviceWaitIdle" => {
                    return Some(unsafe {
                        std::mem::transmute::<vk::PFN_vkDeviceWaitIdle, unsafe extern "system" fn()>(
                            next_device_wait_idle,
                        )
                    });
                }
                b"vkDestroyDevice" => {
                    return Some(unsafe {
                        std::mem::transmute::<vk::PFN_vkDestroyDevice, unsafe extern "system" fn()>(
                            next_destroy_device,
                        )
                    });
                }
                _ => {}
            }
        }
        Some(next_unknown_proc)
    }

    unsafe extern "system" fn next_gdpa_with_swapchain(
        device: vk::Device,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        if !name.is_null() {
            match unsafe { CStr::from_ptr(name) }.to_bytes() {
                b"vkCreateSwapchainKHR" => {
                    return erase_function(next_create_swapchain as vk::PFN_vkCreateSwapchainKHR)
                }
                b"vkDestroySwapchainKHR" => {
                    return erase_function(next_destroy_swapchain as vk::PFN_vkDestroySwapchainKHR)
                }
                b"vkGetSwapchainImagesKHR" => {
                    return erase_function(
                        next_get_swapchain_images as vk::PFN_vkGetSwapchainImagesKHR,
                    )
                }
                b"vkAcquireNextImageKHR" => {
                    return erase_function(next_acquire_next_image as vk::PFN_vkAcquireNextImageKHR)
                }
                b"vkAcquireNextImage2KHR" => {
                    return erase_function(
                        next_acquire_next_image2 as vk::PFN_vkAcquireNextImage2KHR,
                    )
                }
                b"vkQueuePresentKHR" => {
                    return erase_function(next_queue_present as vk::PFN_vkQueuePresentKHR)
                }
                _ => {}
            }
        }
        unsafe { next_gdpa(device, name) }
    }

    unsafe extern "system" fn next_gipa_with_surface(
        instance: vk::Instance,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        if !name.is_null() {
            match unsafe { CStr::from_ptr(name) }.to_bytes() {
                b"vkEnumerateDeviceExtensionProperties" => {
                    return erase_function(
                        next_enumerate_device_extensions
                            as vk::PFN_vkEnumerateDeviceExtensionProperties,
                    )
                }
                b"vkGetPhysicalDeviceSurfaceCapabilitiesKHR" => {
                    return erase_function(
                        next_surface_capabilities
                            as vk::PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR,
                    )
                }
                b"vkGetPhysicalDeviceSurfaceSupportKHR" => {
                    return erase_function(
                        next_surface_support as vk::PFN_vkGetPhysicalDeviceSurfaceSupportKHR,
                    )
                }
                b"vkGetPhysicalDeviceFormatProperties" => {
                    return erase_function(
                        next_format_properties as vk::PFN_vkGetPhysicalDeviceFormatProperties,
                    )
                }
                b"vkGetPhysicalDeviceImageFormatProperties" => {
                    return erase_function(
                        next_image_format_properties
                            as vk::PFN_vkGetPhysicalDeviceImageFormatProperties,
                    )
                }
                _ => {}
            }
        }
        unsafe { next_gipa(instance, name) }
    }

    unsafe extern "system" fn next_enumerate_device_extensions(
        _physical_device: vk::PhysicalDevice,
        layer_name: *const c_char,
        property_count: *mut u32,
        properties: *mut vk::ExtensionProperties,
    ) -> vk::Result {
        DEVICE_EXTENSION_ENUMERATIONS.fetch_add(1, Ordering::SeqCst);
        if !layer_name.is_null() || property_count.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        if properties.is_null() {
            unsafe { *property_count = 1 };
            return vk::Result::SUCCESS;
        }
        if unsafe { *property_count } == 0 {
            return vk::Result::INCOMPLETE;
        }
        let mut property = vk::ExtensionProperties::default();
        for (destination, source) in property
            .extension_name
            .iter_mut()
            .zip(b"VK_KHR_swapchain\0")
        {
            *destination = *source as c_char;
        }
        property.spec_version = 70;
        unsafe {
            *properties = property;
            *property_count = 1;
        }
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_create_swapchain(
        _device: vk::Device,
        create_info: *const vk::SwapchainCreateInfoKHR<'_>,
        _allocator: *const vk::AllocationCallbacks<'_>,
        swapchain: *mut vk::SwapchainKHR,
    ) -> vk::Result {
        let call = SWAPCHAIN_CREATE_CALLS.fetch_add(1, Ordering::SeqCst);
        if !create_info.is_null() {
            let create_info = unsafe { &*create_info };
            let slot = call.min(1);
            SWAPCHAIN_CREATE_USAGES[slot]
                .store(create_info.image_usage.as_raw() as usize, Ordering::SeqCst);
            SWAPCHAIN_CREATE_LAST_USAGE
                .store(create_info.image_usage.as_raw() as usize, Ordering::SeqCst);
            SWAPCHAIN_CREATE_PNEXT.store(create_info.p_next as usize, Ordering::SeqCst);
            if SWAPCHAIN_FAIL_ADDED_USAGE.load(Ordering::SeqCst) != 0
                && create_info
                    .image_usage
                    .contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            {
                return vk::Result::ERROR_FORMAT_NOT_SUPPORTED;
            }
        }
        if !swapchain.is_null() {
            unsafe { *swapchain = vk::SwapchainKHR::from_raw(0xC100 + call as u64) };
        }
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_destroy_swapchain(
        _device: vk::Device,
        _swapchain: vk::SwapchainKHR,
        _allocator: *const vk::AllocationCallbacks<'_>,
    ) {
        SWAPCHAIN_DESTROY_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    unsafe extern "system" fn next_get_swapchain_images(
        _device: vk::Device,
        _swapchain: vk::SwapchainKHR,
        image_count: *mut u32,
        images: *mut vk::Image,
    ) -> vk::Result {
        SWAPCHAIN_IMAGE_CALLS.fetch_add(1, Ordering::SeqCst);
        if image_count.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        if images.is_null() {
            unsafe { *image_count = 2 };
            return vk::Result::SUCCESS;
        }
        let count = unsafe { (*image_count).min(2) };
        for index in 0..count {
            unsafe { *images.add(index as usize) = vk::Image::from_raw(0xD100 + u64::from(index)) };
        }
        unsafe { *image_count = count };
        if count < 2 {
            vk::Result::INCOMPLETE
        } else {
            vk::Result::SUCCESS
        }
    }

    unsafe extern "system" fn next_acquire_next_image(
        _device: vk::Device,
        _swapchain: vk::SwapchainKHR,
        _timeout: u64,
        _semaphore: vk::Semaphore,
        _fence: vk::Fence,
        image_index: *mut u32,
    ) -> vk::Result {
        if !image_index.is_null() {
            unsafe { *image_index = 1 };
        }
        vk::Result::from_raw(ACQUIRE_RESULT.load(Ordering::SeqCst))
    }

    unsafe extern "system" fn next_acquire_next_image2(
        _device: vk::Device,
        acquire_info: *const vk::AcquireNextImageInfoKHR<'_>,
        image_index: *mut u32,
    ) -> vk::Result {
        if !acquire_info.is_null() {
            ACQUIRE2_PNEXT.store(unsafe { (*acquire_info).p_next } as usize, Ordering::SeqCst);
        }
        if !image_index.is_null() {
            unsafe { *image_index = 0 };
        }
        vk::Result::from_raw(ACQUIRE2_RESULT.load(Ordering::SeqCst))
    }

    unsafe extern "system" fn next_queue_present(
        _queue: vk::Queue,
        present_info: *const vk::PresentInfoKHR<'_>,
    ) -> vk::Result {
        if present_info.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        let info = unsafe { &*present_info };
        PRESENT_PNEXT.store(info.p_next as usize, Ordering::SeqCst);
        PRESENT_RESULTS.store(info.p_results as usize, Ordering::SeqCst);
        if !info.p_results.is_null() {
            for index in 0..info.swapchain_count as usize {
                unsafe {
                    *info.p_results.add(index) = if index == 0 {
                        vk::Result::from_raw(PRESENT_FIRST_RESULT.load(Ordering::SeqCst))
                    } else {
                        vk::Result::from_raw(PRESENT_SECOND_RESULT.load(Ordering::SeqCst))
                    }
                };
            }
        }
        vk::Result::from_raw(PRESENT_RESULT.load(Ordering::SeqCst))
    }

    unsafe extern "system" fn next_surface_capabilities(
        _physical_device: vk::PhysicalDevice,
        _surface: vk::SurfaceKHR,
        capabilities: *mut vk::SurfaceCapabilitiesKHR,
    ) -> vk::Result {
        if capabilities.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        unsafe {
            *capabilities = vk::SurfaceCapabilitiesKHR::default().supported_usage_flags(
                (if SURFACE_COLOR_ATTACHMENT_SUPPORTED.load(Ordering::SeqCst) != 0 {
                    vk::ImageUsageFlags::COLOR_ATTACHMENT
                } else {
                    vk::ImageUsageFlags::empty()
                }) | (if SURFACE_TRANSFER_SRC_SUPPORTED.load(Ordering::SeqCst) != 0 {
                    vk::ImageUsageFlags::TRANSFER_SRC
                } else {
                    vk::ImageUsageFlags::empty()
                }),
            )
        };
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_format_properties(
        _physical_device: vk::PhysicalDevice,
        _format: vk::Format,
        properties: *mut vk::FormatProperties,
    ) {
        if properties.is_null() {
            return;
        }
        let mut optimal_tiling_features = vk::FormatFeatureFlags::empty();
        if HDR_UI_IMAGE_SUPPORTED.load(Ordering::SeqCst) != 0 {
            optimal_tiling_features |=
                vk::FormatFeatureFlags::COLOR_ATTACHMENT | vk::FormatFeatureFlags::SAMPLED_IMAGE;
        }
        if HDR_UI_BLEND_SUPPORTED.load(Ordering::SeqCst) != 0 {
            optimal_tiling_features |= vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND;
        }
        unsafe {
            *properties =
                vk::FormatProperties::default().optimal_tiling_features(optimal_tiling_features)
        };
    }

    unsafe extern "system" fn next_image_format_properties(
        _physical_device: vk::PhysicalDevice,
        _format: vk::Format,
        _image_type: vk::ImageType,
        _tiling: vk::ImageTiling,
        usage: vk::ImageUsageFlags,
        _flags: vk::ImageCreateFlags,
        properties: *mut vk::ImageFormatProperties,
    ) -> vk::Result {
        if properties.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        if usage.contains(vk::ImageUsageFlags::TRANSFER_DST)
            && HDR_SCENE_IMAGE_SUPPORTED.load(Ordering::SeqCst) == 0
        {
            return vk::Result::ERROR_FORMAT_NOT_SUPPORTED;
        }
        if usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            && HDR_UI_IMAGE_SUPPORTED.load(Ordering::SeqCst) == 0
        {
            return vk::Result::ERROR_FORMAT_NOT_SUPPORTED;
        }
        unsafe { *properties = vk::ImageFormatProperties::default() };
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_surface_support(
        _physical_device: vk::PhysicalDevice,
        queue_family_index: u32,
        _surface: vk::SurfaceKHR,
        supported: *mut vk::Bool32,
    ) -> vk::Result {
        if supported.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        unsafe {
            *supported = if queue_family_index == 1 {
                vk::TRUE
            } else {
                vk::FALSE
            }
        };
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn gdpa_without_get_device_queue2(
        device: vk::Device,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        if !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkGetDeviceQueue2" {
            return None;
        }
        unsafe { next_gdpa(device, name) }
    }

    unsafe extern "system" fn next_get_device_queue(
        device: vk::Device,
        family_index: u32,
        queue_index: u32,
        queue: *mut vk::Queue,
    ) {
        if !queue.is_null() {
            FORWARDED_QUEUE.store(device.as_raw() as usize, Ordering::Relaxed);
            unsafe {
                *queue = vk::Queue::from_raw(
                    device.as_raw()
                        + 0x1000
                        + u64::from(family_index) * 100
                        + u64::from(queue_index),
                )
            };
        }
    }

    unsafe extern "system" fn next_get_device_queue2(
        device: vk::Device,
        queue_info: *const vk::DeviceQueueInfo2<'_>,
        queue: *mut vk::Queue,
    ) {
        if !queue_info.is_null() && !queue.is_null() {
            let queue_info = unsafe { &*queue_info };
            FORWARDED_QUEUE.store(device.as_raw() as usize, Ordering::Relaxed);
            unsafe {
                *queue = vk::Queue::from_raw(
                    device.as_raw()
                        + 0x2000
                        + u64::from(queue_info.queue_family_index) * 100
                        + u64::from(queue_info.queue_index),
                )
            };
        }
    }

    unsafe extern "system" fn next_queue_submit(
        _queue: vk::Queue,
        _submit_count: u32,
        _submits: *const vk::SubmitInfo<'_>,
        _fence: vk::Fence,
    ) -> vk::Result {
        record_queue_operation();
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_device_wait_idle(_device: vk::Device) -> vk::Result {
        record_queue_operation();
        vk::Result::SUCCESS
    }

    fn record_queue_operation() {
        let active = ACTIVE_QUEUE_OPERATIONS.fetch_add(1, Ordering::SeqCst) + 1;
        MAX_CONCURRENT_QUEUE_OPERATIONS.fetch_max(active, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(10));
        ACTIVE_QUEUE_OPERATIONS.fetch_sub(1, Ordering::SeqCst);
    }

    unsafe extern "system" fn next_queue_submit2(
        _queue: vk::Queue,
        _submit_count: u32,
        _submits: *const vk::SubmitInfo2<'_>,
        _fence: vk::Fence,
    ) -> vk::Result {
        QUEUE_SUBMIT2_CALLS.fetch_add(1, Ordering::SeqCst);
        vk::Result::SUCCESS
    }

    unsafe extern "system" fn next_destroy_device(
        _device: vk::Device,
        _allocator: *const vk::AllocationCallbacks<'_>,
    ) {
        DESTROYED_DEVICES.fetch_add(1, Ordering::SeqCst);
    }

    fn setup_swapchain_device() -> (vk::Device, vk::Queue, vk::SurfaceKHR) {
        static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
        swapchain::clear();
        let id = NEXT_FIXTURE.fetch_add(1, Ordering::SeqCst) as u64 + 1;
        let device = vk::Device::from_raw(0xE000 + id);
        let instance = vk::Instance::from_raw(0xE100 + id);
        let physical_device = vk::PhysicalDevice::from_raw(0xE200 + id);
        let queue = vk::Queue::from_raw(0xE300 + id);
        let surface = vk::SurfaceKHR::from_raw(0xE400 + id);
        let properties = vk::QueueFamilyProperties::default()
            .queue_flags(vk::QueueFlags::GRAPHICS | vk::QueueFlags::TRANSFER)
            .queue_count(1);

        instance_dispatch().insert(instance, next_gipa_with_surface);
        device_dispatch().insert(device, next_gdpa_with_swapchain);
        device_states().insert(
            device,
            DeviceState {
                instance,
                physical_device,
                queue_families: HashMap::from([(
                    1,
                    QueueFamilyState {
                        requested_count: 1,
                        properties,
                    },
                )]),
            },
        );
        queue_dispatch().insert(
            queue,
            QueueState::new(QueueMetadata {
                device,
                instance,
                physical_device,
                family_index: 1,
                queue_index: 0,
                flags: vk::DeviceQueueCreateFlags::empty(),
                family_flags: properties.queue_flags,
            }),
        );
        SWAPCHAIN_CREATE_CALLS.store(0, Ordering::SeqCst);
        SWAPCHAIN_CREATE_USAGES[0].store(0, Ordering::SeqCst);
        SWAPCHAIN_CREATE_USAGES[1].store(0, Ordering::SeqCst);
        SWAPCHAIN_CREATE_LAST_USAGE.store(0, Ordering::SeqCst);
        SWAPCHAIN_CREATE_PNEXT.store(0, Ordering::SeqCst);
        SWAPCHAIN_FAIL_ADDED_USAGE.store(0, Ordering::SeqCst);
        SWAPCHAIN_DESTROY_CALLS.store(0, Ordering::SeqCst);
        SWAPCHAIN_IMAGE_CALLS.store(0, Ordering::SeqCst);
        ACQUIRE_RESULT.store(vk::Result::SUCCESS.as_raw(), Ordering::SeqCst);
        ACQUIRE2_RESULT.store(vk::Result::SUCCESS.as_raw(), Ordering::SeqCst);
        ACQUIRE2_PNEXT.store(0, Ordering::SeqCst);
        PRESENT_RESULT.store(vk::Result::SUCCESS.as_raw(), Ordering::SeqCst);
        PRESENT_FIRST_RESULT.store(vk::Result::SUCCESS.as_raw(), Ordering::SeqCst);
        PRESENT_SECOND_RESULT.store(vk::Result::SUBOPTIMAL_KHR.as_raw(), Ordering::SeqCst);
        PRESENT_PNEXT.store(0, Ordering::SeqCst);
        PRESENT_RESULTS.store(0, Ordering::SeqCst);
        SURFACE_COLOR_ATTACHMENT_SUPPORTED.store(1, Ordering::SeqCst);
        SURFACE_TRANSFER_SRC_SUPPORTED.store(1, Ordering::SeqCst);
        HDR_SCENE_IMAGE_SUPPORTED.store(1, Ordering::SeqCst);
        HDR_UI_IMAGE_SUPPORTED.store(1, Ordering::SeqCst);
        HDR_UI_BLEND_SUPPORTED.store(1, Ordering::SeqCst);
        (device, queue, surface)
    }

    fn cleanup_swapchain_device(device: vk::Device, queue: vk::Queue, _instance: vk::Instance) {
        let instance = device_states().get(&device).map(|state| state.instance);
        swapchain::clear();
        queue_dispatch().remove(&queue);
        device_states().remove(&device);
        device_dispatch().remove(&device);
        if let Some(instance) = instance {
            instance_dispatch().remove(&instance);
        }
    }

    fn base_swapchain_create_info(surface: vk::SurfaceKHR) -> vk::SwapchainCreateInfoKHR<'static> {
        let mut info = vk::SwapchainCreateInfoKHR::default();
        info.surface = surface;
        info.min_image_count = 2;
        info.image_format = vk::Format::B8G8R8A8_UNORM;
        info.image_color_space = vk::ColorSpaceKHR::SRGB_NONLINEAR;
        info.image_extent = vk::Extent2D {
            width: 640,
            height: 480,
        };
        info.image_array_layers = 1;
        info.image_usage = vk::ImageUsageFlags::TRANSFER_DST;
        info.image_sharing_mode = vk::SharingMode::EXCLUSIVE;
        info.present_mode = vk::PresentModeKHR::FIFO;
        info
    }

    unsafe fn call_create_swapchain(
        device: vk::Device,
        info: *const vk::SwapchainCreateInfoKHR<'_>,
    ) -> (vk::Result, vk::SwapchainKHR) {
        let function = unsafe { vkGetDeviceProcAddr(device, c"vkCreateSwapchainKHR".as_ptr()) }
            .expect("swapchain creation proc");
        let create: vk::PFN_vkCreateSwapchainKHR = unsafe { std::mem::transmute_copy(&function) };
        let mut swapchain = vk::SwapchainKHR::null();
        let result = unsafe { create(device, info, std::ptr::null(), &mut swapchain) };
        (result, swapchain)
    }

    #[test]
    fn swapchain_device_commands_are_intercepted_when_downstream_supports_them() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, _) = setup_swapchain_device();
        for name in [
            c"vkCreateSwapchainKHR",
            c"vkDestroySwapchainKHR",
            c"vkGetSwapchainImagesKHR",
            c"vkAcquireNextImageKHR",
            c"vkAcquireNextImage2KHR",
            c"vkQueuePresentKHR",
        ] {
            let layer = unsafe { vkGetDeviceProcAddr(device, name.as_ptr()) }
                .expect("downstream-supported swapchain command");
            let downstream = unsafe { next_gdpa_with_swapchain(device, name.as_ptr()) }
                .expect("fixture swapchain command");
            assert_ne!(
                layer as usize,
                downstream as usize,
                "{}",
                name.to_string_lossy()
            );
        }
        let instance = device_states().get(&device).unwrap().instance;
        for name in [
            c"vkCreateSwapchainKHR",
            c"vkDestroySwapchainKHR",
            c"vkGetSwapchainImagesKHR",
            c"vkAcquireNextImageKHR",
            c"vkAcquireNextImage2KHR",
            c"vkQueuePresentKHR",
        ] {
            let layer = unsafe { vkGetInstanceProcAddr(instance, name.as_ptr()) }
                .expect("instance-supported swapchain command");
            let downstream = unsafe { next_gipa_with_surface(instance, name.as_ptr()) }
                .expect("fixture instance command");
            assert_ne!(
                layer as usize,
                downstream as usize,
                "{}",
                name.to_string_lossy()
            );
        }
        cleanup_swapchain_device(device, queue, vk::Instance::from_raw(0xE101));
    }

    unsafe extern "system" fn gdpa_without_swapchain_create(
        device: vk::Device,
        name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        if !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == b"vkCreateSwapchainKHR"
        {
            return None;
        }
        unsafe { next_gdpa_with_swapchain(device, name) }
    }

    #[test]
    fn swapchain_commands_stay_unavailable_when_downstream_does_not_expose_them() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, _) = setup_swapchain_device();
        device_dispatch().insert(device, gdpa_without_swapchain_create);

        assert!(unsafe { vkGetDeviceProcAddr(device, c"vkCreateSwapchainKHR".as_ptr()) }.is_none());

        cleanup_swapchain_device(device, queue, vk::Instance::from_raw(0));
    }

    #[test]
    fn swapchain_creation_retries_without_added_usage_and_disables_overlay() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        SWAPCHAIN_FAIL_ADDED_USAGE.store(1, Ordering::SeqCst);
        let mut info = base_swapchain_create_info(surface);
        let sentinel = 0x1234usize;
        info.p_next = sentinel as *const c_void;

        let (result, swapchain) = unsafe { call_create_swapchain(device, &info) };

        assert_eq!(result, vk::Result::SUCCESS);
        assert_eq!(SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(
            SWAPCHAIN_CREATE_USAGES[0].load(Ordering::SeqCst) as u32,
            (vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::COLOR_ATTACHMENT).as_raw()
        );
        assert_eq!(
            SWAPCHAIN_CREATE_USAGES[1].load(Ordering::SeqCst) as u32,
            vk::ImageUsageFlags::TRANSFER_DST.as_raw()
        );
        assert_eq!(SWAPCHAIN_CREATE_PNEXT.load(Ordering::SeqCst), sentinel);
        let state = swapchain::state(swapchain).expect("registered fallback swapchain");
        assert!(!state.enabled);
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn swapchain_recreation_with_old_swapchain_skips_usage_retry() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let info = base_swapchain_create_info(surface);
        let (old_result, old_swapchain) = unsafe { call_create_swapchain(device, &info) };
        assert_eq!(old_result, vk::Result::SUCCESS);

        SWAPCHAIN_CREATE_CALLS.store(0, Ordering::SeqCst);
        SWAPCHAIN_FAIL_ADDED_USAGE.store(1, Ordering::SeqCst);
        let mut recreation = base_swapchain_create_info(surface);
        recreation.old_swapchain = old_swapchain;
        let original_usage = recreation.image_usage;
        let (result, new_swapchain) = unsafe { call_create_swapchain(device, &recreation) };

        assert_eq!(result, vk::Result::SUCCESS);
        assert_eq!(SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(
            SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
            original_usage.as_raw()
        );
        assert!(!swapchain::state(new_swapchain).unwrap().enabled);
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn hdr_swapchain_usage_is_added_and_recorded_for_supported_pairs() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let expected_usage = vk::ImageUsageFlags::TRANSFER_DST
            | vk::ImageUsageFlags::COLOR_ATTACHMENT
            | vk::ImageUsageFlags::TRANSFER_SRC;

        for (format, color_space, expected_mode) in [
            (
                vk::Format::A2B10G10R10_UNORM_PACK32,
                vk::ColorSpaceKHR::HDR10_ST2084_EXT,
                crate::hdr::OutputColorMode::Hdr10Pq,
            ),
            (
                vk::Format::R16G16B16A16_SFLOAT,
                vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
                crate::hdr::OutputColorMode::ScRgbLinear,
            ),
        ] {
            let mut info = base_swapchain_create_info(surface);
            info.image_format = format;
            info.image_color_space = color_space;
            let original_usage = info.image_usage;
            let (result, swapchain) = unsafe { call_create_swapchain(device, &info) };

            assert_eq!(result, vk::Result::SUCCESS);
            assert_eq!(
                SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
                expected_usage.as_raw()
            );
            let state = swapchain::state(swapchain).expect("registered HDR swapchain");
            assert!(state.enabled);
            assert_eq!(state.output_color_mode, Some(expected_mode));
            assert!(state.color_space == color_space);
            assert_eq!(state.image_usage.as_raw(), expected_usage.as_raw());
            assert_ne!(original_usage.as_raw(), state.image_usage.as_raw());
        }

        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn hdr_swapchain_requires_surface_transfer_src_support() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        SURFACE_TRANSFER_SRC_SUPPORTED.store(0, Ordering::SeqCst);
        let mut info = base_swapchain_create_info(surface);
        info.image_format = vk::Format::A2B10G10R10_UNORM_PACK32;
        info.image_color_space = vk::ColorSpaceKHR::HDR10_ST2084_EXT;
        let original_usage = info.image_usage;
        let (result, swapchain) = unsafe { call_create_swapchain(device, &info) };

        assert_eq!(result, vk::Result::SUCCESS);
        assert_eq!(SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(
            SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
            original_usage.as_raw()
        );
        let state = swapchain::state(swapchain).expect("registered unsupported HDR swapchain");
        assert!(!state.enabled);
        assert_eq!(state.image_usage.as_raw(), original_usage.as_raw());
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn hdr_swapchain_requires_supported_scene_and_ui_images() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        for (unsupported_resource, scene_supported, ui_image_supported, ui_blend_supported) in
            [(0, 0, 1, 1), (1, 1, 0, 1), (2, 1, 1, 0)]
        {
            HDR_SCENE_IMAGE_SUPPORTED.store(scene_supported, Ordering::SeqCst);
            HDR_UI_IMAGE_SUPPORTED.store(ui_image_supported, Ordering::SeqCst);
            HDR_UI_BLEND_SUPPORTED.store(ui_blend_supported, Ordering::SeqCst);
            let mut info = base_swapchain_create_info(surface);
            info.image_format = vk::Format::A2B10G10R10_UNORM_PACK32;
            info.image_color_space = vk::ColorSpaceKHR::HDR10_ST2084_EXT;
            let original_usage = info.image_usage;
            let (result, swapchain) = unsafe { call_create_swapchain(device, &info) };

            assert_eq!(result, vk::Result::SUCCESS);
            assert_eq!(
                SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst),
                unsupported_resource + 1
            );
            assert_eq!(
                SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
                original_usage.as_raw()
            );
            let state = swapchain::state(swapchain).expect("registered unsupported HDR swapchain");
            assert!(!state.enabled);
            assert_eq!(state.image_usage.as_raw(), original_usage.as_raw());
        }
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn hdr_usage_fallback_and_old_swapchain_follow_safe_creation_policy() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let mut info = base_swapchain_create_info(surface);
        info.image_format = vk::Format::A2B10G10R10_UNORM_PACK32;
        info.image_color_space = vk::ColorSpaceKHR::HDR10_ST2084_EXT;
        let original_usage = info.image_usage;
        SWAPCHAIN_FAIL_ADDED_USAGE.store(1, Ordering::SeqCst);
        let (fallback_result, fallback_swapchain) = unsafe { call_create_swapchain(device, &info) };

        assert_eq!(fallback_result, vk::Result::SUCCESS);
        assert_eq!(SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(
            SWAPCHAIN_CREATE_USAGES[0].load(Ordering::SeqCst) as u32,
            (original_usage
                | vk::ImageUsageFlags::COLOR_ATTACHMENT
                | vk::ImageUsageFlags::TRANSFER_SRC)
                .as_raw()
        );
        assert_eq!(
            SWAPCHAIN_CREATE_USAGES[1].load(Ordering::SeqCst) as u32,
            original_usage.as_raw()
        );
        let fallback = swapchain::state(fallback_swapchain).expect("registered fallback swapchain");
        assert!(!fallback.enabled);
        assert_eq!(fallback.image_usage.as_raw(), original_usage.as_raw());

        SWAPCHAIN_FAIL_ADDED_USAGE.store(0, Ordering::SeqCst);
        let mut old_info = base_swapchain_create_info(surface);
        old_info.image_format = vk::Format::A2B10G10R10_UNORM_PACK32;
        old_info.image_color_space = vk::ColorSpaceKHR::HDR10_ST2084_EXT;
        let (old_result, old_swapchain) = unsafe { call_create_swapchain(device, &old_info) };
        assert_eq!(old_result, vk::Result::SUCCESS);
        let mut recreation = old_info;
        recreation.old_swapchain = old_swapchain;
        let (recreate_result, new_swapchain) =
            unsafe { call_create_swapchain(device, &recreation) };

        assert_eq!(recreate_result, vk::Result::SUCCESS);
        assert_eq!(SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst), 4);
        assert_eq!(
            SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
            recreation.image_usage.as_raw()
        );
        let recreated = swapchain::state(new_swapchain).expect("registered recreated swapchain");
        assert!(!recreated.enabled);
        assert_eq!(
            recreated.image_usage.as_raw(),
            recreation.image_usage.as_raw()
        );
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn unsupported_swapchain_modes_are_forwarded_and_disabled() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let mut layered = base_swapchain_create_info(surface);
        layered.image_array_layers = 2;
        let mut hdr = base_swapchain_create_info(surface);
        hdr.image_color_space = vk::ColorSpaceKHR::HDR10_ST2084_EXT;
        let mut protected = base_swapchain_create_info(surface);
        protected.flags = vk::SwapchainCreateFlagsKHR::PROTECTED;
        let mut shared = base_swapchain_create_info(surface);
        shared.present_mode = vk::PresentModeKHR::SHARED_DEMAND_REFRESH;

        for info in [layered, hdr, protected, shared] {
            let original_usage = info.image_usage;
            let (result, swapchain) = unsafe { call_create_swapchain(device, &info) };
            assert_eq!(result, vk::Result::SUCCESS);
            assert_eq!(
                SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
                original_usage.as_raw()
            );
            let state = swapchain::state(swapchain).expect("registered unsupported swapchain");
            assert!(!state.enabled);
        }
        let mut display_p3 = base_swapchain_create_info(surface);
        display_p3.image_color_space = vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT;
        let (_, display_p3_swapchain) = unsafe { call_create_swapchain(device, &display_p3) };
        assert!(swapchain::state(display_p3_swapchain).unwrap().enabled);

        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn swapchain_without_color_attachment_support_is_forwarded_unchanged() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        SURFACE_COLOR_ATTACHMENT_SUPPORTED.store(0, Ordering::SeqCst);
        let info = base_swapchain_create_info(surface);
        let (result, swapchain) = unsafe { call_create_swapchain(device, &info) };

        assert_eq!(result, vk::Result::SUCCESS);
        assert_eq!(SWAPCHAIN_CREATE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(
            SWAPCHAIN_CREATE_LAST_USAGE.load(Ordering::SeqCst) as u32,
            info.image_usage.as_raw()
        );
        let state = swapchain::state(swapchain).expect("registered unsupported swapchain");
        assert!(!state.enabled);
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn concurrent_swapchain_requires_the_present_queue_family() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let families = [0u32];
        let mut info = base_swapchain_create_info(surface);
        info.image_sharing_mode = vk::SharingMode::CONCURRENT;
        info.queue_family_index_count = families.len() as u32;
        info.p_queue_family_indices = families.as_ptr();
        let (_, swapchain) = unsafe { call_create_swapchain(device, &info) };

        let state = swapchain::state(swapchain).expect("registered concurrent swapchain");
        assert!(!state.enabled);
        assert!(!swapchain::is_eligible_for_queue(swapchain, queue));

        let supported_families = [1u32];
        let mut supported_info = base_swapchain_create_info(surface);
        supported_info.image_sharing_mode = vk::SharingMode::CONCURRENT;
        supported_info.queue_family_index_count = supported_families.len() as u32;
        supported_info.p_queue_family_indices = supported_families.as_ptr();
        let (_, supported_swapchain) = unsafe { call_create_swapchain(device, &supported_info) };
        assert!(swapchain::state(supported_swapchain).unwrap().enabled);
        assert!(swapchain::is_eligible_for_queue(supported_swapchain, queue));
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn acquire_apis_and_swapchain_images_preserve_driver_results_and_indices() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let mut info = base_swapchain_create_info(surface);
        info.image_usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
        let (create_result, swapchain) = unsafe { call_create_swapchain(device, &info) };
        assert_eq!(create_result, vk::Result::SUCCESS);
        assert!(swapchain::state(swapchain).unwrap().enabled);
        assert!(swapchain::is_eligible_for_queue(swapchain, queue));

        let get_images =
            unsafe { vkGetDeviceProcAddr(device, c"vkGetSwapchainImagesKHR".as_ptr()) }
                .expect("image enumeration proc");
        let get_images: vk::PFN_vkGetSwapchainImagesKHR =
            unsafe { std::mem::transmute_copy(&get_images) };
        let mut image_count = 2;
        let mut images = [vk::Image::null(); 2];
        assert_eq!(
            unsafe { get_images(device, swapchain, &mut image_count, images.as_mut_ptr()) },
            vk::Result::SUCCESS
        );
        assert_eq!(image_count, 2);
        assert_eq!(swapchain::state(swapchain).unwrap().images, images);
        assert_eq!(SWAPCHAIN_IMAGE_CALLS.load(Ordering::SeqCst), 1);

        ACQUIRE_RESULT.store(vk::Result::SUBOPTIMAL_KHR.as_raw(), Ordering::SeqCst);
        let acquire = unsafe { vkGetDeviceProcAddr(device, c"vkAcquireNextImageKHR".as_ptr()) }
            .expect("acquire proc");
        let acquire: vk::PFN_vkAcquireNextImageKHR = unsafe { std::mem::transmute_copy(&acquire) };
        let mut image_index = u32::MAX;
        assert_eq!(
            unsafe {
                acquire(
                    device,
                    swapchain,
                    77,
                    vk::Semaphore::from_raw(0xFA01),
                    vk::Fence::from_raw(0xFA02),
                    &mut image_index,
                )
            },
            vk::Result::SUBOPTIMAL_KHR
        );
        assert_eq!(image_index, 1);
        assert!(swapchain::state(swapchain)
            .unwrap()
            .acquired_images
            .contains(&1));

        let mut partial_count = 1;
        let mut partial_images = [vk::Image::null(); 1];
        assert_eq!(
            unsafe {
                get_images(
                    device,
                    swapchain,
                    &mut partial_count,
                    partial_images.as_mut_ptr(),
                )
            },
            vk::Result::INCOMPLETE
        );
        let state = swapchain::state(swapchain).unwrap();
        assert_eq!(state.images, images);
        assert!(state.acquired_images.contains(&1));

        let sentinel = 0x8765usize;
        let acquire_info = vk::AcquireNextImageInfoKHR::default()
            .swapchain(swapchain)
            .timeout(91)
            .semaphore(vk::Semaphore::from_raw(0xFA03))
            .fence(vk::Fence::from_raw(0xFA04));
        let mut acquire_info = acquire_info;
        acquire_info.p_next = sentinel as *const c_void;
        ACQUIRE2_RESULT.store(vk::Result::ERROR_OUT_OF_DATE_KHR.as_raw(), Ordering::SeqCst);
        let acquire2 = unsafe { vkGetDeviceProcAddr(device, c"vkAcquireNextImage2KHR".as_ptr()) }
            .expect("acquire2 proc");
        let acquire2: vk::PFN_vkAcquireNextImage2KHR =
            unsafe { std::mem::transmute_copy(&acquire2) };
        image_index = u32::MAX;
        assert_eq!(
            unsafe { acquire2(device, &acquire_info, &mut image_index) },
            vk::Result::ERROR_OUT_OF_DATE_KHR
        );
        assert_eq!(image_index, 0);
        assert_eq!(ACQUIRE2_PNEXT.load(Ordering::SeqCst), sentinel);
        assert!(!swapchain::state(swapchain)
            .unwrap()
            .acquired_images
            .contains(&0));

        let destroy = unsafe { vkGetDeviceProcAddr(device, c"vkDestroySwapchainKHR".as_ptr()) }
            .expect("destroy proc");
        let destroy: vk::PFN_vkDestroySwapchainKHR = unsafe { std::mem::transmute_copy(&destroy) };
        unsafe { destroy(device, swapchain, std::ptr::null()) };
        assert!(swapchain::state(swapchain).is_none());
        assert_eq!(SWAPCHAIN_DESTROY_CALLS.load(Ordering::SeqCst), 1);
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn batched_present_preserves_pnext_results_and_overall_result() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (device, queue, surface) = setup_swapchain_device();
        let instance = vk::Instance::from_raw(0xE101);
        let info = base_swapchain_create_info(surface);
        let (_, first) = unsafe { call_create_swapchain(device, &info) };
        let (_, second) = unsafe { call_create_swapchain(device, &info) };
        let swapchains = [first, second];
        let image_indices = [1, 1];

        let get_images =
            unsafe { vkGetDeviceProcAddr(device, c"vkGetSwapchainImagesKHR".as_ptr()) }
                .expect("image enumeration proc");
        let get_images: vk::PFN_vkGetSwapchainImagesKHR =
            unsafe { std::mem::transmute_copy(&get_images) };
        for swapchain in swapchains {
            let mut count = 2;
            let mut images = [vk::Image::null(); 2];
            assert_eq!(
                unsafe { get_images(device, swapchain, &mut count, images.as_mut_ptr()) },
                vk::Result::SUCCESS
            );
        }
        let acquire = unsafe { vkGetDeviceProcAddr(device, c"vkAcquireNextImageKHR".as_ptr()) }
            .expect("acquire proc");
        let acquire: vk::PFN_vkAcquireNextImageKHR = unsafe { std::mem::transmute_copy(&acquire) };
        for swapchain in swapchains {
            let mut acquired_index = u32::MAX;
            assert_eq!(
                unsafe {
                    acquire(
                        device,
                        swapchain,
                        u64::MAX,
                        vk::Semaphore::null(),
                        vk::Fence::null(),
                        &mut acquired_index,
                    )
                },
                vk::Result::SUCCESS
            );
            assert_eq!(acquired_index, 1);
        }
        let waits = [vk::Semaphore::from_raw(0xFB01)];
        let mut results = [vk::Result::NOT_READY; 2];
        let sentinel = 0x9876usize;
        let mut present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        present_info.p_next = sentinel as *const c_void;
        present_info.p_results = results.as_mut_ptr();
        PRESENT_RESULT.store(vk::Result::ERROR_OUT_OF_DATE_KHR.as_raw(), Ordering::SeqCst);
        PRESENT_SECOND_RESULT.store(vk::Result::ERROR_OUT_OF_DATE_KHR.as_raw(), Ordering::SeqCst);

        let present = unsafe { vkGetDeviceProcAddr(device, c"vkQueuePresentKHR".as_ptr()) }
            .expect("present proc");
        let present: vk::PFN_vkQueuePresentKHR = unsafe { std::mem::transmute_copy(&present) };
        assert_eq!(
            unsafe { present(queue, &present_info) },
            vk::Result::ERROR_OUT_OF_DATE_KHR
        );
        assert_eq!(PRESENT_PNEXT.load(Ordering::SeqCst), sentinel);
        assert_eq!(
            PRESENT_RESULTS.load(Ordering::SeqCst),
            results.as_mut_ptr() as usize
        );
        assert_eq!(
            results,
            [vk::Result::SUCCESS, vk::Result::ERROR_OUT_OF_DATE_KHR]
        );
        let first_state = swapchain::state(first).unwrap();
        // Rendering setup is unavailable in this stubbed Vulkan fixture, so the layer
        // disables only its overlay while still forwarding the successful present.
        assert!(!first_state.enabled);
        assert!(!first_state.acquired_images.contains(&1));
        assert!(first_state.presented_images.contains(&1));
        assert!(!first_state.uncertain_images.contains(&1));
        let second_state = swapchain::state(second).unwrap();
        assert!(!second_state.enabled);
        assert!(!second_state.acquired_images.contains(&1));
        assert!(!second_state.presented_images.contains(&1));
        assert!(second_state.uncertain_images.contains(&1));

        let mut reacquired_index = u32::MAX;
        assert_eq!(
            unsafe {
                acquire(
                    device,
                    first,
                    u64::MAX,
                    vk::Semaphore::null(),
                    vk::Fence::null(),
                    &mut reacquired_index,
                )
            },
            vk::Result::SUCCESS
        );
        assert_eq!(reacquired_index, 1);
        let reacquired_state = swapchain::state(first).unwrap();
        assert!(reacquired_state.acquired_images.contains(&1));
        assert!(!reacquired_state.presented_images.contains(&1));
        assert!(!reacquired_state.uncertain_images.contains(&1));
        cleanup_swapchain_device(device, queue, instance);
    }

    #[test]
    fn negotiates_the_highest_supported_loader_interface() {
        let mut interface = NegotiateLayerInterface {
            s_type: NEGOTIATE_INTERFACE_STRUCT,
            p_next: std::ptr::null_mut(),
            loader_layer_interface_version: 99,
            pfn_get_instance_proc_addr: vkGetInstanceProcAddr,
            pfn_get_device_proc_addr: vkGetDeviceProcAddr,
            pfn_get_physical_device_proc_addr: None,
        };

        let result = unsafe { vkNegotiateLoaderLayerInterfaceVersion(&mut interface) };

        assert_eq!(result, vk::Result::SUCCESS);
        assert_eq!(interface.loader_layer_interface_version, 2);
        assert!(interface.pfn_get_physical_device_proc_addr.is_none());
        assert!(unsafe {
            (interface.pfn_get_instance_proc_addr)(
                vk::Instance::null(),
                c"vkCreateInstance".as_ptr(),
            )
        }
        .is_some());
        assert!(unsafe {
            (interface.pfn_get_device_proc_addr)(
                vk::Device::null(),
                c"vkGetDeviceProcAddr".as_ptr(),
            )
        }
        .is_some());
    }

    #[test]
    fn rejects_unsupported_loader_interface_versions() {
        let mut interface = NegotiateLayerInterface {
            s_type: NEGOTIATE_INTERFACE_STRUCT,
            p_next: std::ptr::null_mut(),
            loader_layer_interface_version: 0,
            pfn_get_instance_proc_addr: vkGetInstanceProcAddr,
            pfn_get_device_proc_addr: vkGetDeviceProcAddr,
            pfn_get_physical_device_proc_addr: None,
        };

        assert_eq!(
            unsafe { vkNegotiateLoaderLayerInterfaceVersion(&mut interface) },
            vk::Result::ERROR_INITIALIZATION_FAILED
        );
    }

    #[test]
    fn enumerates_the_layer_with_vulkan_count_and_incomplete_semantics() {
        let mut count = 0;
        assert_eq!(
            unsafe { vkEnumerateInstanceLayerProperties(&mut count, std::ptr::null_mut()) },
            vk::Result::SUCCESS
        );
        assert_eq!(count, 1);

        let mut properties = [vk::LayerProperties::default()];
        count = 1;
        assert_eq!(
            unsafe { vkEnumerateInstanceLayerProperties(&mut count, properties.as_mut_ptr()) },
            vk::Result::SUCCESS
        );
        assert_eq!(count, 1);
        assert_eq!(
            properties[0]
                .layer_name_as_c_str()
                .unwrap()
                .to_bytes_with_nul(),
            VK_LAYER_NAME
        );
        assert_eq!(
            properties[0]
                .description_as_c_str()
                .unwrap()
                .to_bytes_with_nul(),
            VK_LAYER_DESCRIPTION
        );

        count = 0;
        assert_eq!(
            unsafe { vkEnumerateInstanceLayerProperties(&mut count, properties.as_mut_ptr()) },
            vk::Result::INCOMPLETE
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn reports_no_layer_extensions_and_rejects_other_layer_names() {
        let mut count = 7;
        assert_eq!(
            unsafe {
                vkEnumerateInstanceExtensionProperties(
                    VK_LAYER_NAME.as_ptr().cast(),
                    &mut count,
                    std::ptr::null_mut(),
                )
            },
            vk::Result::SUCCESS
        );
        assert_eq!(count, 0);

        assert_eq!(
            unsafe {
                vkEnumerateInstanceExtensionProperties(
                    c"VK_LAYER_OTHER".as_ptr(),
                    &mut count,
                    std::ptr::null_mut(),
                )
            },
            vk::Result::ERROR_LAYER_NOT_PRESENT
        );
        assert_eq!(
            unsafe {
                vkEnumerateInstanceExtensionProperties(
                    std::ptr::null(),
                    &mut count,
                    std::ptr::null_mut(),
                )
            },
            vk::Result::ERROR_LAYER_NOT_PRESENT
        );
    }

    #[test]
    fn forwards_device_extension_enumeration_when_no_layer_name_is_requested() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let instance = vk::Instance::from_raw(0xEA01);
        let physical_device = vk::PhysicalDevice::from_raw(0xEA02);
        instance_dispatch().insert(instance, next_gipa_with_surface);
        physical_device_instances().insert(physical_device, instance);
        DEVICE_EXTENSION_ENUMERATIONS.store(0, Ordering::SeqCst);

        let mut count = 0;
        assert_eq!(
            unsafe {
                vkEnumerateDeviceExtensionProperties(
                    physical_device,
                    std::ptr::null(),
                    &mut count,
                    std::ptr::null_mut(),
                )
            },
            vk::Result::SUCCESS
        );
        assert_eq!(count, 1);

        let mut properties = [vk::ExtensionProperties::default()];
        count = 1;
        assert_eq!(
            unsafe {
                vkEnumerateDeviceExtensionProperties(
                    physical_device,
                    std::ptr::null(),
                    &mut count,
                    properties.as_mut_ptr(),
                )
            },
            vk::Result::SUCCESS
        );
        assert_eq!(count, 1);
        assert_eq!(
            properties[0].extension_name_as_c_str().unwrap(),
            c"VK_KHR_swapchain"
        );
        assert_eq!(DEVICE_EXTENSION_ENUMERATIONS.load(Ordering::SeqCst), 2);

        physical_device_instances().remove(&physical_device);
        instance_dispatch().remove(&instance);
    }

    #[repr(C)]
    struct Sentinel {
        s_type: vk::StructureType,
        p_next: *const c_void,
    }

    #[test]
    fn create_instance_advances_only_loader_link_info_and_preserves_pnext_nodes() {
        let sentinel_after = Sentinel {
            s_type: vk::StructureType::APPLICATION_INFO,
            p_next: std::ptr::null(),
        };
        let mut next_link = LayerInstanceLink {
            p_next: std::ptr::null_mut(),
            pfn_next_get_instance_proc_addr: recording_gipa,
            pfn_next_get_physical_device_proc_addr: None,
        };
        let mut current_link = LayerInstanceLink {
            p_next: &mut next_link,
            pfn_next_get_instance_proc_addr: recording_gipa,
            pfn_next_get_physical_device_proc_addr: None,
        };
        let loader_info = LayerInstanceCreateInfo {
            s_type: vk::StructureType::LOADER_INSTANCE_CREATE_INFO,
            p_next: &sentinel_after as *const _ as *const c_void,
            function: 0,
            u: LayerInstanceUnion {
                p_layer_info: &mut current_link,
            },
        };
        let sentinel_before = Sentinel {
            s_type: vk::StructureType::APPLICATION_INFO,
            p_next: &loader_info as *const _ as *const c_void,
        };
        let mut create_info = vk::InstanceCreateInfo::default();
        create_info.p_next = &sentinel_before as *const _ as *const c_void;
        let original_p_next = create_info.p_next;
        let mut instance = vk::Instance::null();

        assert_eq!(
            unsafe { vkCreateInstance(&create_info, std::ptr::null(), &mut instance,) },
            vk::Result::SUCCESS
        );
        assert_eq!(instance.as_raw(), 0x1234);
        assert_eq!(create_info.p_next, original_p_next);
        assert_eq!(
            loader_info.p_next,
            &sentinel_after as *const _ as *const c_void
        );
        assert!(std::ptr::eq(
            unsafe { loader_info.u.p_layer_info },
            &mut next_link
        ));

        FORWARDED_INSTANCE.store(0, Ordering::Relaxed);
        assert!(unsafe { vkGetInstanceProcAddr(instance, c"vkUnknownForTest".as_ptr()) }.is_some());
        assert_eq!(
            FORWARDED_INSTANCE.load(Ordering::Relaxed),
            instance.as_raw() as usize
        );
        instance_dispatch().remove(&instance);
    }

    #[test]
    fn create_device_uses_physical_device_from_group_enumeration() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let instance = vk::Instance::from_raw(0x4455);
        instance_dispatch().insert(instance, next_gipa);
        let enumerate_groups =
            unsafe { vkGetInstanceProcAddr(instance, c"vkEnumeratePhysicalDeviceGroups".as_ptr()) }
                .expect("intercepted physical device group enumeration");
        let enumerate_groups: vk::PFN_vkEnumeratePhysicalDeviceGroups =
            unsafe { std::mem::transmute_copy(&enumerate_groups) };
        let mut group_count = 1;
        let mut groups = [vk::PhysicalDeviceGroupProperties::default()];
        assert_eq!(
            unsafe { enumerate_groups(instance, &mut group_count, groups.as_mut_ptr()) },
            vk::Result::SUCCESS
        );
        let physical_device = groups[0].physical_devices[0];
        assert_eq!(physical_device.as_raw(), 0x5566);
        assert_eq!(
            physical_device_instances().get(&physical_device),
            Some(&instance)
        );
        let enumerate_groups_khr = unsafe {
            vkGetInstanceProcAddr(instance, c"vkEnumeratePhysicalDeviceGroupsKHR".as_ptr())
        }
        .expect("intercepted KHR physical device group enumeration");
        let enumerate_groups_khr: vk::PFN_vkEnumeratePhysicalDeviceGroups =
            unsafe { std::mem::transmute_copy(&enumerate_groups_khr) };
        group_count = 1;
        groups[0] = vk::PhysicalDeviceGroupProperties::default();
        assert_eq!(
            unsafe { enumerate_groups_khr(instance, &mut group_count, groups.as_mut_ptr()) },
            vk::Result::SUCCESS
        );
        assert_eq!(
            physical_device_instances().get(&groups[0].physical_devices[1]),
            Some(&instance)
        );

        let sentinel_after = Sentinel {
            s_type: vk::StructureType::APPLICATION_INFO,
            p_next: std::ptr::null(),
        };
        let mut next_link = LayerDeviceLink {
            p_next: std::ptr::null_mut(),
            pfn_next_get_instance_proc_addr: next_gipa,
            pfn_next_get_device_proc_addr: next_gdpa,
        };
        let mut current_link = LayerDeviceLink {
            p_next: &mut next_link,
            pfn_next_get_instance_proc_addr: next_gipa,
            pfn_next_get_device_proc_addr: next_gdpa,
        };
        let loader_info = LayerDeviceCreateInfo {
            s_type: vk::StructureType::LOADER_DEVICE_CREATE_INFO,
            p_next: &sentinel_after as *const _ as *const c_void,
            function: 0,
            u: LayerDeviceUnion {
                p_layer_info: &mut current_link,
            },
        };
        let sentinel_before = Sentinel {
            s_type: vk::StructureType::APPLICATION_INFO,
            p_next: &loader_info as *const _ as *const c_void,
        };
        let priorities = [1.0, 0.5];
        let queue_create_info = vk::DeviceQueueCreateInfo::default()
            .queue_family_index(1)
            .queue_priorities(&priorities);
        let queue_create_infos = [queue_create_info];
        let mut create_info =
            vk::DeviceCreateInfo::default().queue_create_infos(&queue_create_infos);
        create_info.p_next = &sentinel_before as *const _ as *const c_void;
        let original_p_next = create_info.p_next;
        let mut device = vk::Device::null();

        assert_eq!(
            unsafe { vkCreateDevice(physical_device, &create_info, std::ptr::null(), &mut device) },
            vk::Result::SUCCESS
        );
        assert_eq!(device.as_raw(), 0x7788);
        let device_state = device_states().get(&device).cloned().expect("device state");
        assert_eq!(device_state.instance, instance);
        assert_eq!(device_state.physical_device, physical_device);
        assert_eq!(device_state.queue_families[&1].requested_count, 2);
        assert!(device_state.queue_families[&1]
            .properties
            .queue_flags
            .contains(
                vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER
            ));
        assert_eq!(create_info.p_next, original_p_next);
        assert_eq!(
            loader_info.p_next,
            &sentinel_after as *const _ as *const c_void
        );
        assert!(std::ptr::eq(
            unsafe { loader_info.u.p_layer_info },
            &mut next_link
        ));

        FORWARDED_DEVICE.store(0, Ordering::Relaxed);
        assert!(unsafe { vkGetDeviceProcAddr(device, c"vkUnknownForTest".as_ptr()) }.is_some());
        assert_eq!(
            FORWARDED_DEVICE.load(Ordering::Relaxed),
            device.as_raw() as usize
        );

        let mut queue = vk::Queue::null();
        unsafe { vkGetDeviceQueue(device, 1, 1, &mut queue) };
        let metadata = queue_metadata(queue).expect("queue metadata");
        assert_eq!(metadata.instance, instance);
        assert_eq!(metadata.physical_device, physical_device);
        assert_eq!(metadata.family_index, 1);
        assert!(metadata.family_flags.contains(vk::QueueFlags::GRAPHICS));

        device_dispatch().remove(&device);
        device_states().remove(&device);
        queue_dispatch().remove(&queue);
        physical_device_instances().remove(&physical_device);
        instance_dispatch().remove(&instance);
    }

    #[test]
    fn get_device_queue2_is_not_exposed_when_downstream_does_not_support_it() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = vk::Device::from_raw(0xA008);
        device_dispatch().insert(device, gdpa_without_get_device_queue2);

        assert!(unsafe { vkGetDeviceProcAddr(device, c"vkGetDeviceQueue2".as_ptr()) }.is_none());

        device_dispatch().remove(&device);
    }

    #[test]
    fn get_device_queue_forwards_and_records_device_family_and_index() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = vk::Device::from_raw(0xA001);
        device_dispatch().insert(device, next_gdpa);
        let get_queue = unsafe { vkGetDeviceProcAddr(device, c"vkGetDeviceQueue".as_ptr()) }
            .expect("intercepted vkGetDeviceQueue");
        let get_queue: vk::PFN_vkGetDeviceQueue = unsafe { std::mem::transmute_copy(&get_queue) };
        let mut queue = vk::Queue::null();

        unsafe { get_queue(device, 3, 2, &mut queue) };

        assert_eq!(
            FORWARDED_QUEUE.load(Ordering::Relaxed),
            device.as_raw() as usize
        );
        assert!(!queue.is_null());
        let metadata = queue_metadata(queue).expect("queue registry entry");
        assert_eq!(metadata.device, device);
        assert_eq!(metadata.family_index, 3);
        assert_eq!(metadata.queue_index, 2);

        device_dispatch().remove(&device);
        queue_dispatch().remove(&queue);
    }

    #[test]
    fn get_device_queue2_is_intercepted_and_records_queue_flags() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = vk::Device::from_raw(0xA002);
        device_dispatch().insert(device, next_gdpa);
        let get_queue = unsafe { vkGetDeviceProcAddr(device, c"vkGetDeviceQueue2".as_ptr()) }
            .expect("intercepted vkGetDeviceQueue2");
        let get_queue: vk::PFN_vkGetDeviceQueue2 = unsafe { std::mem::transmute_copy(&get_queue) };
        let queue_info = vk::DeviceQueueInfo2::default()
            .flags(vk::DeviceQueueCreateFlags::PROTECTED)
            .queue_family_index(5)
            .queue_index(1);
        let mut queue = vk::Queue::null();

        unsafe { get_queue(device, &queue_info, &mut queue) };

        assert_eq!(
            FORWARDED_QUEUE.load(Ordering::Relaxed),
            device.as_raw() as usize
        );
        let metadata = queue_metadata(queue).expect("queue registry entry");
        assert_eq!(metadata.device, device);
        assert_eq!(metadata.family_index, 5);
        assert_eq!(metadata.queue_index, 1);
        assert!(metadata.flags == vk::DeviceQueueCreateFlags::PROTECTED);

        device_dispatch().remove(&device);
        queue_dispatch().remove(&queue);
    }

    #[test]
    fn queue_submissions_on_the_same_queue_are_serialized() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = vk::Device::from_raw(0xA003);
        let queue = vk::Queue::from_raw(0xB003);
        device_dispatch().insert(device, next_gdpa);
        queue_dispatch().insert(
            queue,
            QueueState::new(QueueMetadata::new(
                device,
                0,
                0,
                vk::DeviceQueueCreateFlags::empty(),
            )),
        );
        ACTIVE_QUEUE_OPERATIONS.store(0, Ordering::SeqCst);
        MAX_CONCURRENT_QUEUE_OPERATIONS.store(0, Ordering::SeqCst);

        let threads = (0..2)
            .map(|_| {
                std::thread::spawn(move || unsafe {
                    vkQueueSubmit(queue, 0, std::ptr::null(), vk::Fence::null())
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), vk::Result::SUCCESS);
        }

        assert_eq!(MAX_CONCURRENT_QUEUE_OPERATIONS.load(Ordering::SeqCst), 1);
        device_dispatch().remove(&device);
        queue_dispatch().remove(&queue);
    }

    #[test]
    fn device_wait_idle_serializes_with_queue_submissions() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = vk::Device::from_raw(0xA007);
        let queue = vk::Queue::from_raw(0xB007);
        device_dispatch().insert(device, next_gdpa);
        queue_dispatch().insert(
            queue,
            QueueState::new(QueueMetadata::new(
                device,
                0,
                0,
                vk::DeviceQueueCreateFlags::empty(),
            )),
        );
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        ACTIVE_QUEUE_OPERATIONS.store(0, Ordering::SeqCst);
        MAX_CONCURRENT_QUEUE_OPERATIONS.store(0, Ordering::SeqCst);

        let submit_barrier = std::sync::Arc::clone(&barrier);
        let submit_thread = std::thread::spawn(move || {
            submit_barrier.wait();
            unsafe { vkQueueSubmit(queue, 0, std::ptr::null(), vk::Fence::null()) }
        });
        let idle_barrier = std::sync::Arc::clone(&barrier);
        let idle_thread = std::thread::spawn(move || {
            idle_barrier.wait();
            unsafe { vkDeviceWaitIdle(device) }
        });
        barrier.wait();

        assert_eq!(submit_thread.join().unwrap(), vk::Result::SUCCESS);
        assert_eq!(idle_thread.join().unwrap(), vk::Result::SUCCESS);
        assert_eq!(MAX_CONCURRENT_QUEUE_OPERATIONS.load(Ordering::SeqCst), 1);

        device_dispatch().remove(&device);
        queue_dispatch().remove(&queue);
    }

    #[test]
    fn queue_submit2_khr_intercepts_and_forwards_only_supported_alias() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let device = vk::Device::from_raw(0xA006);
        let queue = vk::Queue::from_raw(0xB006);
        device_dispatch().insert(device, next_gdpa);
        queue_dispatch().insert(
            queue,
            QueueState::new(QueueMetadata::new(
                device,
                0,
                0,
                vk::DeviceQueueCreateFlags::empty(),
            )),
        );
        QUEUE_SUBMIT2_CALLS.store(0, Ordering::SeqCst);

        assert!(unsafe { vkGetDeviceProcAddr(device, c"vkQueueSubmit2".as_ptr()) }.is_none());
        let submit = unsafe { vkGetDeviceProcAddr(device, c"vkQueueSubmit2KHR".as_ptr()) }
            .expect("supported KHR submit2 interception");
        let submit: vk::PFN_vkQueueSubmit2 = unsafe { std::mem::transmute_copy(&submit) };
        assert_eq!(
            unsafe { submit(queue, 0, std::ptr::null(), vk::Fence::null()) },
            vk::Result::SUCCESS
        );
        assert_eq!(QUEUE_SUBMIT2_CALLS.load(Ordering::SeqCst), 1);

        device_dispatch().remove(&device);
        queue_dispatch().remove(&queue);
    }

    #[test]
    fn multiple_devices_keep_queue_state_isolated_and_destroy_cleans_it() {
        let _guard = DISPATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first_device = vk::Device::from_raw(0xA004);
        let second_device = vk::Device::from_raw(0xA005);
        device_dispatch().insert(first_device, next_gdpa);
        device_dispatch().insert(second_device, next_gdpa);
        let mut first_queue = vk::Queue::null();
        let mut second_queue = vk::Queue::null();

        unsafe {
            vkGetDeviceQueue(first_device, 1, 0, &mut first_queue);
            vkGetDeviceQueue(second_device, 1, 0, &mut second_queue);
        }

        assert_ne!(first_queue, second_queue);
        assert_eq!(queue_metadata(first_queue).unwrap().device, first_device);
        assert_eq!(queue_metadata(second_queue).unwrap().device, second_device);
        DESTROYED_DEVICES.store(0, Ordering::SeqCst);
        unsafe { vkDestroyDevice(first_device, std::ptr::null()) };
        assert_eq!(DESTROYED_DEVICES.load(Ordering::SeqCst), 1);
        assert!(!device_dispatch().contains_key(&first_device));
        assert!(!device_states().contains_key(&first_device));
        assert!(queue_metadata(first_queue).is_none());
        assert!(queue_metadata(second_queue).is_some());

        unsafe { vkDestroyDevice(second_device, std::ptr::null()) };
        assert!(queue_metadata(second_queue).is_none());
    }
}
