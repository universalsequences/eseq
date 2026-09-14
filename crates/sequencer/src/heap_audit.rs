//! Direct Rust GlobalAlloc accounting for real-time playback and regression tests.
//! This deliberately does not claim to intercept allocations inside C/DSP.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[derive(serde::Serialize)]
pub struct Counts {
    pub allocations: usize,
    pub deallocations: usize,
    pub reallocations: usize,
}

thread_local! {
    static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) };
}

struct TrackingAllocator;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn record(operation: usize, update: impl FnOnce(&mut Counts)) {
    #[cfg(feature = "audio-heap-audit")]
    playback::record(operation);
    // Allocators must not panic, including during thread-local teardown.
    let _ = COUNTS.try_with(|cell| {
        if let Some(mut counts) = cell.get() {
            update(&mut counts);
            cell.set(Some(counts));
        }
    });
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(0, |counts| counts.allocations = counts.allocations.saturating_add(1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(0, |counts| counts.allocations = counts.allocations.saturating_add(1));
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(1, |counts| counts.deallocations = counts.deallocations.saturating_add(1));
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(2, |counts| counts.reallocations = counts.reallocations.saturating_add(1));
        unsafe { System.realloc(ptr, layout, size) }
    }
}

/// Count only work done by `f` on the current thread. Set up fixtures before
/// entering the scope, and destroy any callback-local owners inside it.
pub(crate) fn measure<T>(f: impl FnOnce() -> T) -> (T, Counts) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNTS.with(|cell| cell.set(None));
        }
    }
    COUNTS.with(|cell| {
        assert!(cell.get().is_none(), "allocation measurements cannot be nested");
        cell.set(Some(Counts::default()));
    });
    let reset = Reset;
    let result = f();
    let counts = COUNTS.with(|cell| cell.get().unwrap());
    drop(reset);
    (result, counts)
}

#[cfg(feature = "audio-heap-audit")]
pub use playback::{AudioScope, begin, end, snapshot, calibrate};

#[cfg(feature = "audio-heap-audit")]
mod playback {
    use super::Counts;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    thread_local! {
        static DEPTH: Cell<u32> = const { Cell::new(0) };
        static WORKER: Cell<bool> = const { Cell::new(false) };
    }
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static COUNTS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
    static CALLBACKS: AtomicU64 = AtomicU64::new(0);
    static WORKER_ENTRIES: AtomicU64 = AtomicU64::new(0);

    pub(super) fn record(operation: usize) {
        if !ENABLED.load(Ordering::Acquire) { return; }
        // These const-initialized TLS cells need no allocation or destructor.
        // try_with also makes allocator calls during thread teardown harmless.
        let _ = DEPTH.try_with(|depth| {
            if depth.get() == 0 { return; }
            let _ = WORKER.try_with(|worker| {
                COUNTS[operation + if worker.get() { 3 } else { 0 }]
                    .fetch_add(1, Ordering::Relaxed);
            });
        });
    }

    pub struct AudioScope(std::marker::PhantomData<*mut ()>);
    impl AudioScope {
        pub fn enter() -> Self {
            DEPTH.with(|depth| {
                if depth.get() == 0 && ENABLED.load(Ordering::Acquire) {
                    CALLBACKS.fetch_add(1, Ordering::Relaxed);
                }
                depth.set(depth.get() + 1);
            });
            Self(std::marker::PhantomData)
        }
    }
    impl Drop for AudioScope {
        fn drop(&mut self) { DEPTH.with(|depth| depth.set(depth.get() - 1)); }
    }

    // The native engine calls these at worker thread entry/exit. Rust DSP
    // invoked by C therefore uses the same allocator counters as the callback.
    #[no_mangle]
    pub extern "C" fn eseq_rust_audio_heap_worker_enter() {
        WORKER.with(|worker| worker.set(true));
        DEPTH.with(|depth| depth.set(depth.get() + 1));
        WORKER_ENTRIES.fetch_add(1, Ordering::Relaxed);
    }
    #[no_mangle]
    pub extern "C" fn eseq_rust_audio_heap_worker_exit() {
        DEPTH.with(|depth| depth.set(depth.get() - 1));
        WORKER.with(|worker| worker.set(false));
    }

    /// Run only with counting disabled; enable after loading and warmup.
    pub fn begin() {
        for count in &COUNTS { count.store(0, Ordering::Relaxed); }
        CALLBACKS.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Release);
    }
    pub fn end() { ENABLED.store(false, Ordering::Release); }

    #[derive(Debug, serde::Serialize)]
    pub struct Report {
        pub callback: Counts,
        pub workers: Counts,
        pub callbacks_entered: u64,
        pub worker_threads_entered: u64,
    }
    /// Read after audio threads have stopped to include in-flight counters.
    pub fn snapshot() -> Report {
        let counts = |offset: usize| Counts {
            allocations: COUNTS[offset].load(Ordering::Relaxed) as usize,
            deallocations: COUNTS[offset + 1].load(Ordering::Relaxed) as usize,
            reallocations: COUNTS[offset + 2].load(Ordering::Relaxed) as usize,
        };
        Report { callback: counts(0), workers: counts(3),
            callbacks_entered: CALLBACKS.load(Ordering::Relaxed),
            worker_threads_entered: WORKER_ENTRIES.load(Ordering::Relaxed) }
    }

    /// Exercise every Rust allocator entry point and both thread roles before
    /// interpreting playback counts. Unmarked scheduler/UI work must be ignored.
    pub fn calibrate() -> Result<Report, &'static str> {
        fn allocate() {
            unsafe {
                use std::alloc::{alloc, alloc_zeroed, dealloc, realloc, Layout};
                let layout = Layout::from_size_align(256, 16).unwrap();
                let first = std::hint::black_box(alloc(layout));
                let second = std::hint::black_box(alloc_zeroed(layout));
                assert!(!first.is_null() && !second.is_null());
                first.write_volatile(42);
                let first = std::hint::black_box(realloc(first, layout, 512));
                assert!(!first.is_null());
                dealloc(first, Layout::from_size_align(512, 16).unwrap());
                dealloc(second, layout);
            }
        }
        begin();
        allocate(); // Unmarked work is outside the audio scope.
        {
            let _scope = AudioScope::enter();
            let _nested = AudioScope::enter();
            allocate();
        }
        std::thread::spawn(|| {
            eseq_rust_audio_heap_worker_enter();
            allocate();
            eseq_rust_audio_heap_worker_exit();
        }).join().map_err(|_| "Rust allocator worker calibration panicked")?;
        end();
        let report = snapshot();
        let expected = Counts { allocations: 2, deallocations: 2, reallocations: 1 };
        if report.callback != expected || report.workers != expected
            || report.callbacks_entered != 1 || report.worker_threads_entered != 1 {
            return Err("Rust allocator calibration failed");
        }
        WORKER_ENTRIES.store(0, Ordering::Relaxed);
        Ok(report)
    }
}
