//! Loader ABI, instance dispatch, and Vulkan layer-chain traversal.
use ash::vk::{self, Handle};
use std::{
    collections::HashMap,
    ffi::{c_char, c_void, CStr},
    panic::AssertUnwindSafe,
    sync::{Mutex, MutexGuard, OnceLock},
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
static INSTANCE_DISPATCH: OnceLock<Mutex<InstanceDispatch>> = OnceLock::new();
static PHYSICAL_DEVICE_INSTANCES: OnceLock<Mutex<PhysicalDeviceInstances>> = OnceLock::new();
static DEVICE_DISPATCH: OnceLock<Mutex<DeviceDispatch>> = OnceLock::new();

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

    // Pass the next layer's link node down before forwarding device creation.
    unsafe { (*loader_info).u.p_layer_info = (*link).p_next };
    let result = unsafe { create_next(physical_device, create_info, allocator, device) };
    if result == vk::Result::SUCCESS && !unsafe { (*device).is_null() } {
        device_dispatch().insert(unsafe { *device }, next_gdpa);
    }
    result
}

unsafe fn destroy_device(device: vk::Device, allocator: *const vk::AllocationCallbacks<'_>) {
    if let Some(next) = next_device_proc_addr(device) {
        if let Some(function) = unsafe { next(device, c"vkDestroyDevice".as_ptr()) } {
            let destroy: vk::PFN_vkDestroyDevice = unsafe { std::mem::transmute_copy(&function) };
            unsafe { destroy(device, allocator) };
        }
    }
    device_dispatch().remove(&device);
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

    unsafe extern "system" fn next_gdpa(
        device: vk::Device,
        _name: *const c_char,
    ) -> vk::PFN_vkVoidFunction {
        FORWARDED_DEVICE.store(device.as_raw() as usize, Ordering::Relaxed);
        Some(next_unknown_proc)
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
    fn create_device_advances_device_link_and_enables_unknown_gdpa_forwarding() {
        let instance = vk::Instance::from_raw(0x4455);
        let physical_device = vk::PhysicalDevice::from_raw(0x5566);
        instance_dispatch().insert(instance, next_gipa);
        physical_device_instances().insert(physical_device, instance);

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
        let mut create_info = vk::DeviceCreateInfo::default();
        create_info.p_next = &sentinel_before as *const _ as *const c_void;
        let original_p_next = create_info.p_next;
        let mut device = vk::Device::null();

        assert_eq!(
            unsafe { vkCreateDevice(physical_device, &create_info, std::ptr::null(), &mut device) },
            vk::Result::SUCCESS
        );
        assert_eq!(device.as_raw(), 0x7788);
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

        device_dispatch().remove(&device);
        physical_device_instances().remove(&physical_device);
        instance_dispatch().remove(&instance);
    }
}
