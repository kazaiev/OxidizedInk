//! The global allocator: esp-alloc's heap, with large blocks put in PSRAM.
//! esp-alloc hands out its regions in order, internal RAM first, so the
//! reader's big buffers (font caches, chapters) used to fill the internal
//! RAM that Wi-Fi needs. Blocks of `BIG` bytes or more now go to PSRAM
//! when it has room; smaller ones stay in the faster internal RAM.

use core::alloc::{GlobalAlloc, Layout};

use esp_alloc::{MemoryCapability, HEAP};

const BIG: usize = 16 * 1024;

struct Heap;

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= BIG {
            let ptr = unsafe { HEAP.alloc_caps(MemoryCapability::External.into(), layout) };
            if !ptr.is_null() {
                return ptr;
            }
        }
        unsafe { GlobalAlloc::alloc(&HEAP, layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { GlobalAlloc::dealloc(&HEAP, ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Heap = Heap;
