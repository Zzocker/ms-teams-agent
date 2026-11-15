// Included only in the temporary Linux/glibc profiling binary.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct ProfileAllocator;
static PROFILE_LIVE: AtomicUsize = AtomicUsize::new(0);
static PROFILE_PEAK: AtomicUsize = AtomicUsize::new(0);
static PROFILE_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PROFILE_FREED: AtomicUsize = AtomicUsize::new(0);

fn profile_allocated(bytes: usize) {
    PROFILE_ALLOCATED.fetch_add(bytes, Relaxed);
    let live = PROFILE_LIVE.fetch_add(bytes, Relaxed) + bytes;
    PROFILE_PEAK.fetch_max(live, Relaxed);
}

unsafe impl GlobalAlloc for ProfileAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() { profile_allocated(layout.size()); }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() { profile_allocated(layout.size()); }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        PROFILE_LIVE.fetch_sub(layout.size(), Relaxed);
        PROFILE_FREED.fetch_add(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(ptr, layout, size) };
        if !next.is_null() {
            // Count successful reallocations as freeing the old block and allocating the new one.
            PROFILE_LIVE.fetch_sub(layout.size(), Relaxed);
            PROFILE_FREED.fetch_add(layout.size(), Relaxed);
            profile_allocated(size);
        }
        next
    }
}

#[global_allocator]
static PROFILE_ALLOCATOR: ProfileAllocator = ProfileAllocator;

#[repr(C)]
#[derive(Default)]
struct ProfileMallinfo {
    arena: usize, ordblks: usize, smblks: usize, hblks: usize, hblkhd: usize,
    usmblks: usize, fsmblks: usize, uordblks: usize, fordblks: usize, keepcost: usize,
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
compile_error!("The allocation profiler requires Linux with glibc");

unsafe extern "C" {
    fn mallinfo2() -> ProfileMallinfo;
    fn malloc_trim(pad: usize) -> i32;
}

fn profile_allocations() -> serde_json::Value {
    let heap = unsafe { mallinfo2() };
    serde_json::json!({
        "rust_live_bytes": PROFILE_LIVE.load(Relaxed),
        "rust_peak_bytes": PROFILE_PEAK.load(Relaxed),
        "rust_allocated_bytes": PROFILE_ALLOCATED.load(Relaxed),
        "rust_freed_bytes": PROFILE_FREED.load(Relaxed),
        "glibc_used_bytes": heap.uordblks + heap.hblkhd,
        "glibc_free_arena_bytes": heap.fordblks,
        "glibc_arena_bytes": heap.arena,
        "glibc_mmap_bytes": heap.hblkhd,
    })
}

#[cfg(test)]
mod profile_allocator_tests {
    use super::*;

    #[test]
    fn allocation_reallocation_and_free_are_counted() {
        // Direct calls isolate the assertions from the test harness's allocator activity.
        let before = PROFILE_ALLOCATED.load(Relaxed);
        let freed = PROFILE_FREED.load(Relaxed);
        unsafe {
            let layout = Layout::from_size_align(64, 8).unwrap();
            let ptr = PROFILE_ALLOCATOR.alloc_zeroed(layout);
            assert!(!ptr.is_null());
            let ptr = PROFILE_ALLOCATOR.realloc(ptr, layout, 128);
            assert!(!ptr.is_null());
            PROFILE_ALLOCATOR.dealloc(ptr, Layout::from_size_align(128, 8).unwrap());
        }
        assert!(PROFILE_ALLOCATED.load(Relaxed) >= before + 192);
        assert!(PROFILE_FREED.load(Relaxed) >= freed + 192);
    }
}
