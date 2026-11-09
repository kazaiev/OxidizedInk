//! Buffers kept for good in PSRAM, so they leave internal RAM to the radio.

use alloc::vec::Vec;
use core::alloc::Layout;

use esp_alloc::{MemoryCapability, HEAP};

/// A slice of `len` values made by `init`, in PSRAM if there is room,
/// otherwise wherever the allocator puts it.
pub fn slice<T>(len: usize, init: impl Fn() -> T) -> &'static mut [T] {
    let layout = Layout::array::<T>(len).unwrap();
    let ptr = unsafe { HEAP.alloc_caps(MemoryCapability::External.into(), layout) } as *mut T;
    if ptr.is_null() {
        return (0..len).map(|_| init()).collect::<Vec<T>>().leak();
    }
    for i in 0..len {
        unsafe { ptr.add(i).write(init()) };
    }
    unsafe { core::slice::from_raw_parts_mut(ptr, len) }
}
