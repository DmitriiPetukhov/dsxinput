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
struct QueueMetadata {
    device: vk::Device,
    instance: vk::Instance,
    physical_device: vk::PhysicalDevice,
    family_index: u32,
    queue_index: u32,
    flags: vk::DeviceQueueCreateFlags,
    family_flags: vk::QueueFlags,
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
    _physical_device: vk::PhysicalDevice,
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
    let next = instance_dispatch().get(&instance).copied();
    if let Some(next) = next {
        let name = c"vkDestroyInstance";
        if let Some(function) = unsafe { next(instance, name.as_ptr()) } {
            let destroy: vk::PFN_vkDestroyInstance = unsafe { std::mem::transmute_copy(&function) };
            unsafe { destroy(instance, allocator) };
        }
    }
    instance_dispatch().remove(&instance);
    physical_device_instances().retain(|_, physical_instance| *physical_instance != instance);
}

fn next_instance_proc_addr(instance: vk::Instance) -> Option<vk::PFN_vkGetInstanceProcAddr> {
    let dispatch = instance_dispatch();
    dispatch.get(&instance).copied().or_else(|| {
        if instance.is_null() {
            dispatch.values().next().copied()
        } else {
            None
        }
    })
}

fn next_device_proc_addr(device: vk::Device) -> Option<vk::PFN_vkGetDeviceProcAddr> {
    device_dispatch().get(&device).copied()
}

unsafe fn next_device_function(device: vk::Device, name: &CStr) -> vk::PFN_vkVoidFunction {
    let Some(next) = next_device_proc_addr(device) else {
        return None;
    };
    unsafe { next(device, name.as_ptr()) }
}

fn queue_metadata(queue: vk::Queue) -> Option<QueueMetadata> {
    queue_dispatch().get(&queue).map(|state| state.metadata)
}

fn with_queue_serialization<T>(queue: vk::Queue, call: impl FnOnce() -> T) -> T {
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

fn with_device_queue_serialization<T>(device: vk::Device, call: impl FnOnce() -> T) -> T {
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

    if let Some(next) = next_device_proc_addr(device) {
        if let Some(function) = unsafe { next(device, c"vkDestroyDevice".as_ptr()) } {
            let destroy: vk::PFN_vkDestroyDevice = unsafe { std::mem::transmute_copy(&function) };
            unsafe { destroy(device, allocator) };
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
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
    static DISPATCH_TEST_LOCK: Mutex<()> = Mutex::new(());

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
