//! Kernel heap: an address-sorted free list with coalescing.
//!
//! Every allocation is rounded up to a 16-byte multiple and aligned to at
//! least 16 bytes. That invariant is what keeps the allocator leak-free: any
//! padding produced when honouring a larger alignment is itself a multiple of
//! 16, so it is either zero or big enough to return to the free list.
//!
//! When the list cannot satisfy a request the heap grows by mapping fresh
//! frames, up to `HEAP_MAX`.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use crate::mm::paging;
use crate::sync::SpinLock;

const MIN_BLOCK: usize = 16;

#[repr(C)]
struct FreeRegion {
    size: usize,
    next: *mut FreeRegion,
}

pub struct Heap {
    head: *mut FreeRegion,
    /// Virtual address just past the last mapped heap byte.
    brk: u64,
    mapped: u64,
    allocated: usize,
    live_allocations: usize,
    total_allocations: u64,
}

unsafe impl Send for Heap {}

impl Heap {
    pub const fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            brk: paging::HEAP_BASE,
            mapped: 0,
            allocated: 0,
            live_allocations: 0,
            total_allocations: 0,
        }
    }

    /// Insert a region, keeping the list sorted by address and merging with
    /// any neighbour it touches.
    unsafe fn insert(&mut self, addr: usize, size: usize) {
        debug_assert!(size >= MIN_BLOCK && size % MIN_BLOCK == 0);
        let region = addr as *mut FreeRegion;
        (*region).size = size;
        (*region).next = ptr::null_mut();

        // Find the insertion point.
        let mut previous: *mut FreeRegion = ptr::null_mut();
        let mut current = self.head;
        while !current.is_null() && (current as usize) < addr {
            previous = current;
            current = (*current).next;
        }

        (*region).next = current;
        if previous.is_null() {
            self.head = region;
        } else {
            (*previous).next = region;
        }

        // Merge forward, then backward.
        if !current.is_null() && addr + size == current as usize {
            (*region).size += (*current).size;
            (*region).next = (*current).next;
        }
        if !previous.is_null() && previous as usize + (*previous).size == addr {
            (*previous).size += (*region).size;
            (*previous).next = (*region).next;
        }
    }

    fn normalise(layout: Layout) -> (usize, usize) {
        let align = layout.align().max(MIN_BLOCK);
        let size = layout.size().max(MIN_BLOCK);
        let size = (size + MIN_BLOCK - 1) & !(MIN_BLOCK - 1);
        (size, align)
    }

    unsafe fn take(&mut self, size: usize, align: usize) -> *mut u8 {
        let mut previous: *mut FreeRegion = ptr::null_mut();
        let mut current = self.head;

        while !current.is_null() {
            let start = current as usize;
            let region_size = (*current).size;
            let aligned = (start + align - 1) & !(align - 1);
            let front = aligned - start;

            if front + size <= region_size {
                let back = region_size - front - size;
                let next = (*current).next;

                // Unlink, then give back whatever the allocation did not use.
                if previous.is_null() {
                    self.head = next;
                } else {
                    (*previous).next = next;
                }
                if front > 0 {
                    self.insert(start, front);
                }
                if back > 0 {
                    self.insert(aligned + size, back);
                }

                self.allocated += size;
                self.live_allocations += 1;
                self.total_allocations += 1;
                return aligned as *mut u8;
            }

            previous = current;
            current = (*current).next;
        }
        ptr::null_mut()
    }

    /// Map more frames onto the end of the heap.
    unsafe fn grow(&mut self, at_least: u64) -> bool {
        // Always a power of two of at least HEAP_INITIAL, so it is inherently
        // page-aligned and growth stays amortised.
        let step = at_least.max(paging::HEAP_INITIAL).next_power_of_two();
        if step > paging::HEAP_MAX - self.mapped {
            return false;
        }

        let mut frames = super::FRAMES.lock();
        let pml4 = paging::active_pml4();
        if paging::map_anonymous(pml4, self.brk, step, &mut frames).is_err() {
            return false;
        }
        drop(frames);

        let start = self.brk as usize;
        self.brk += step;
        self.mapped += step;
        self.insert(start, step as usize);
        true
    }

    pub unsafe fn init(&mut self) {
        assert!(self.mapped == 0, "heap already initialised");
        let ok = self.grow(paging::HEAP_INITIAL);
        assert!(ok, "could not map the initial kernel heap");
    }

    pub fn stats(&self) -> HeapStats {
        let mut free_bytes = 0usize;
        let mut free_blocks = 0usize;
        let mut largest = 0usize;
        unsafe {
            let mut current = self.head;
            while !current.is_null() {
                free_bytes += (*current).size;
                largest = largest.max((*current).size);
                free_blocks += 1;
                current = (*current).next;
            }
        }
        HeapStats {
            mapped: self.mapped,
            allocated: self.allocated,
            free_bytes,
            free_blocks,
            largest_free: largest,
            live_allocations: self.live_allocations,
            total_allocations: self.total_allocations,
        }
    }
}

#[derive(Clone, Copy)]
pub struct HeapStats {
    pub mapped: u64,
    pub allocated: usize,
    pub free_bytes: usize,
    pub free_blocks: usize,
    pub largest_free: usize,
    pub live_allocations: usize,
    pub total_allocations: u64,
}

pub struct LockedHeap(SpinLock<Heap>);

impl LockedHeap {
    pub const fn new() -> Self {
        Self(SpinLock::new(Heap::new()))
    }

    pub fn init(&self) {
        unsafe { self.0.lock().init() }
    }

    pub fn stats(&self) -> HeapStats {
        self.0.lock().stats()
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (size, align) = Heap::normalise(layout);
        let mut heap = self.0.lock();
        let pointer = heap.take(size, align);
        if !pointer.is_null() {
            return pointer;
        }
        // Grow by enough to satisfy this request even in the worst alignment case.
        if heap.grow(size as u64 + align as u64) {
            return heap.take(size, align);
        }
        ptr::null_mut()
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if pointer.is_null() {
            return;
        }
        let (size, _) = Heap::normalise(layout);
        let mut heap = self.0.lock();
        heap.allocated -= size;
        heap.live_allocations -= 1;
        heap.insert(pointer as usize, size);
    }
}

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::new();
