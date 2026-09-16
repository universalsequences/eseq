//! Reusable storage for retained Metal geometry.
//!
//! A cache entry and every frame that encodes it own immutable buffer leases.
//! Dropping the last lease returns storage to the free list. Frame leases are
//! released only after command-buffer completion, including when a cache entry
//! is replaced or evicted before the GPU executes its draw. Metal's own retained
//! references protect object lifetime; these leases additionally protect bytes
//! from CPU overwrite.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLBuffer, MTLCommandBuffer, MTLCommandBufferStatus, MTLDevice, MTLResourceOptions};

type MetalBuffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type CommandBuffer = Retained<ProtocolObject<dyn MTLCommandBuffer>>;

static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct BufferPoolStats {
    pub allocations: u64,
    pub allocated_bytes: usize,
    pub reuses: u64,
    pub uploaded_bytes: usize,
    pub backpressure_waits: u64,
    pub backpressure_wait_time: Duration,
}

#[derive(Clone)]
pub(super) struct BufferLease(Rc<LeasedBuffer>);

struct LeasedBuffer {
    buffer: Option<MetalBuffer>,
    pool: Weak<RefCell<PoolState>>,
    last_pinned_frame: Cell<u64>,
}

impl BufferLease {
    pub fn metal(&self) -> &ProtocolObject<dyn MTLBuffer> {
        self.0.buffer.as_deref().expect("live buffer lease")
    }

    pub fn capacity(&self) -> usize { self.metal().length() }
}

impl Drop for LeasedBuffer {
    fn drop(&mut self) {
        let Some(pool) = self.pool.upgrade() else { return; };
        let buffer = self.buffer.take().expect("leased storage");
        let capacity = buffer.length();
        let mut state = pool.borrow_mut();
        if capacity <= state.max_spare_bytes.saturating_sub(state.spare_bytes)
            && state.spare_count < state.max_spare_buffers
        {
            state.spare_bytes += capacity;
            state.spare_count += 1;
            state.spares.entry(capacity).or_default().push(buffer);
        }
        // Excess spare storage is released, never held without a budget.
    }
}

struct FrameBuffers {
    id: u64,
    leases: RefCell<Vec<BufferLease>>,
}

struct InFlightFrame {
    command_buffer: CommandBuffer,
    // Drop outside a PoolState borrow: the last lease returns its buffer there.
    _leases: Vec<BufferLease>,
}

struct PoolState {
    spares: BTreeMap<usize, Vec<MetalBuffer>>,
    spare_bytes: usize,
    spare_count: usize,
    max_spare_bytes: usize,
    max_spare_buffers: usize,
    max_in_flight: usize,
    in_flight: VecDeque<InFlightFrame>,
    active_frame: Weak<FrameBuffers>,
    stats: BufferPoolStats,
}

pub(super) struct MetalBufferPool {
    state: Rc<RefCell<PoolState>>,
}

/// An encoding scope. Early returns drop its leases without submission. The
/// only submission method commits the command buffer and transfers the leases
/// to the completion queue together, so error paths cannot forget to pin them.
pub(super) struct BufferFrame {
    pool: Rc<RefCell<PoolState>>,
    buffers: Rc<FrameBuffers>,
    command_buffer: CommandBuffer,
}

impl BufferFrame {
    pub fn submit(self) {
        let leases = std::mem::take(&mut *self.buffers.leases.borrow_mut());
        self.command_buffer.commit();
        self.pool.borrow_mut().in_flight.push_back(InFlightFrame {
            command_buffer: self.command_buffer.clone(), _leases: leases,
        });
    }
}

impl MetalBufferPool {
    pub fn new() -> Self {
        Self::with_limits(64 * 1024 * 1024, 8192, 3)
    }

    fn with_limits(max_spare_bytes: usize, max_spare_buffers: usize, max_in_flight: usize) -> Self {
        assert!(max_in_flight > 0);
        Self { state: Rc::new(RefCell::new(PoolState {
            spares: BTreeMap::new(), spare_bytes: 0, spare_count: 0,
            max_spare_bytes, max_spare_buffers, max_in_flight,
            in_flight: VecDeque::new(), active_frame: Weak::new(),
            stats: BufferPoolStats::default(),
        })) }
    }

    fn reclaim_completed(&self) {
        loop {
            let completed = {
                let mut state = self.state.borrow_mut();
                if state.in_flight.front().is_some_and(|frame| matches!(
                    frame.command_buffer.status(),
                    MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
                )) {
                    state.in_flight.pop_front()
                } else { None }
            };
            let Some(completed) = completed else { break; };
            drop(completed);
        }
    }

    pub fn begin_frame(&self, command_buffer: CommandBuffer) -> BufferFrame {
        assert!(self.state.borrow().active_frame.upgrade().is_none(), "overlapping CPU encoding scopes");
        self.reclaim_completed();
        let oldest = {
            let mut state = self.state.borrow_mut();
            if state.in_flight.len() >= state.max_in_flight {
                state.stats.backpressure_waits += 1;
                state.in_flight.front().map(|frame| frame.command_buffer.clone())
            } else { None }
        };
        if let Some(oldest) = oldest {
            // At most three submitted frames own storage. Unlike growing a ring
            // indefinitely, this applies backpressure when the GPU falls behind.
            let started = Instant::now();
            oldest.waitUntilCompleted();
            self.state.borrow_mut().stats.backpressure_wait_time += started.elapsed();
            self.reclaim_completed();
        }
        let buffers = Rc::new(FrameBuffers {
            id: NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed),
            leases: RefCell::new(Vec::new()),
        });
        self.state.borrow_mut().active_frame = Rc::downgrade(&buffers);
        BufferFrame { pool: Rc::clone(&self.state), buffers, command_buffer }
    }

    /// Keep a draw's immutable storage alive through this frame's completion.
    /// Multiple phases/clips drawing the same lease pin it only once per frame.
    pub fn pin(&self, lease: &BufferLease) {
        assert!(Weak::ptr_eq(&lease.0.pool, &Rc::downgrade(&self.state)), "foreign buffer pool");
        let frame = self.state.borrow().active_frame.upgrade().expect("draw outside an encoding scope");
        if lease.0.last_pinned_frame.replace(frame.id) != frame.id {
            frame.leases.borrow_mut().push(lease.clone());
        }
    }

    /// Upload an immutable copy. Only exclusively owned, unleased spare storage
    /// is writable here; active/cache/in-flight buffers never enter the free list.
    pub fn upload<T: Copy>(&self, device: &ProtocolObject<dyn MTLDevice>, data: &[T]) -> Option<BufferLease> {
        let byte_len = std::mem::size_of_val(data);
        if byte_len == 0 { return None; }
        let capacity = byte_len.max(256).checked_next_power_of_two()?;
        let spare = {
            let mut state = self.state.borrow_mut();
            let spare = state.spares.get_mut(&capacity).and_then(Vec::pop);
            if spare.is_some() {
                state.spare_bytes -= capacity;
                state.spare_count -= 1;
                state.stats.reuses += 1;
            }
            if state.spares.get(&capacity).is_some_and(Vec::is_empty) {
                state.spares.remove(&capacity);
            }
            spare
        };
        let buffer = if let Some(spare) = spare { spare } else {
            let buffer = device.newBufferWithLength_options(capacity,
                MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined)?;
            let mut state = self.state.borrow_mut();
            state.stats.allocations += 1;
            state.stats.allocated_bytes += capacity;
            buffer
        };
        // `data` is copied while borrowed; no host pointer escapes. The spare
        // has no leases and the new lease is published only after this write.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(),
                buffer.contents().as_ptr().cast::<u8>(), byte_len);
        }
        self.state.borrow_mut().stats.uploaded_bytes += byte_len;
        Some(BufferLease(Rc::new(LeasedBuffer {
            buffer: Some(buffer), pool: Rc::downgrade(&self.state),
            last_pinned_frame: Cell::new(0),
        })))
    }

    pub fn stats(&self) -> BufferPoolStats { self.state.borrow().stats }
    pub fn spare_bytes(&self) -> usize { self.state.borrow().spare_bytes }
    pub fn in_flight_frames(&self) -> usize { self.state.borrow().in_flight.len() }

    /// Fault injection for the renderer's GPU-fenced pixel regression. Removing
    /// frame ownership must produce corrupted earlier frames while CPU/cache
    /// ownership alone appears valid. This method does not exist in app builds.
    #[cfg(test)]
    pub fn discard_in_flight_leases_for_test(&self) {
        let leases: Vec<_> = self.state.borrow_mut().in_flight.iter_mut()
            .map(|frame| std::mem::take(&mut frame._leases)).collect();
        drop(leases);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_metal::{MTLBlitCommandEncoder, MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLEvent, MTLSharedEvent};

    fn device() -> Retained<ProtocolObject<dyn MTLDevice>> {
        MTLCreateSystemDefaultDevice().expect("Metal device")
    }

    fn address(lease: &BufferLease) -> usize {
        lease.metal() as *const _ as *const () as usize
    }

    fn bytes(buffer: &ProtocolObject<dyn MTLBuffer>, len: usize) -> Vec<u8> {
        unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), len).to_vec() }
    }

    #[test]
    fn storage_reuses_capacity_only_after_last_owner_drops() {
        let device = device();
        let pool = MetalBufferPool::new();
        let first = pool.upload(&device, &[0x11u8; 300]).unwrap();
        assert_eq!(first.capacity(), 512);
        let original_address = address(&first);
        let cached = first.clone();
        drop(first);
        let second = pool.upload(&device, &[0x22u8; 400]).unwrap();
        assert_ne!(address(&second), original_address, "cache ownership prohibits overwrite");
        assert_eq!(bytes(cached.metal(), 300), vec![0x11; 300]);
        drop(cached);
        let third = pool.upload(&device, &[0x33u8; 350]).unwrap();
        assert_eq!(address(&third), original_address);
        assert_eq!(bytes(third.metal(), 350), vec![0x33; 350]);
        assert_eq!(pool.stats().allocations, 2);
        assert_eq!(pool.stats().reuses, 1);
        let grown = pool.upload(&device, &[0x44u8; 900]).unwrap();
        assert_eq!(grown.capacity(), 1024);
        assert_eq!(bytes(second.metal(), 400), vec![0x22; 400]);
    }

    #[test]
    fn aborted_frame_releases_its_pins_and_spares_are_bounded() {
        let device = device();
        let queue = device.newCommandQueue().unwrap();
        let pool = MetalBufferPool::with_limits(512, 2, 3);
        let command = queue.commandBuffer().unwrap();
        let frame = pool.begin_frame(command.clone());
        let buffer = pool.upload(&device, &[1u8; 200]).unwrap();
        let original_address = address(&buffer);
        pool.pin(&buffer);
        pool.pin(&buffer);
        assert_eq!(frame.buffers.leases.borrow().len(), 1, "pin once across repeated draws");
        drop(buffer);
        assert_eq!(pool.state.borrow().spare_bytes, 0);
        drop(frame);
        assert_eq!(command.status(), MTLCommandBufferStatus::NotEnqueued);
        assert!(pool.state.borrow().in_flight.is_empty());
        let buffer = pool.upload(&device, &[2u8; 200]).unwrap();
        assert_eq!(address(&buffer), original_address);
        let second = pool.upload(&device, &[3u8; 200]).unwrap();
        let third = pool.upload(&device, &[4u8; 200]).unwrap();
        drop((buffer, second, third));
        assert_eq!(pool.state.borrow().spare_bytes, 512);
        assert_eq!(pool.state.borrow().spare_count, 2);
        let oversized = pool.upload(&device, &[5u8; 2048]).unwrap();
        drop(oversized);
        assert_eq!(pool.state.borrow().spare_bytes, 512);
    }

    struct ReleaseGate(Retained<ProtocolObject<dyn MTLSharedEvent>>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) { self.0.setSignaledValue(u64::MAX); }
    }

    fn copy_to_readback(device: &ProtocolObject<dyn MTLDevice>, command: &CommandBuffer,
        pool: &MetalBufferPool, source: &BufferLease) -> MetalBuffer
    {
        let result = device.newBufferWithLength_options(256, MTLResourceOptions::StorageModeShared).unwrap();
        pool.pin(source);
        let encoder = command.blitCommandEncoder().unwrap();
        unsafe { encoder.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(
            source.metal(), 0, &result, 0, 256); }
        encoder.endEncoding();
        result
    }

    #[test]
    fn storage_waits_for_every_gpu_reader_after_cache_eviction() {
        let device = device();
        let queue = device.newCommandQueue().unwrap();
        let pool = MetalBufferPool::new();
        let gate = ReleaseGate(device.newSharedEvent().expect("shared event"));
        let event: &ProtocolObject<dyn MTLEvent> = ProtocolObject::from_ref(&*gate.0);
        let original = pool.upload(&device, &[0x11u8; 256]).unwrap();
        let original_address = address(&original);

        let first = queue.commandBuffer().unwrap();
        let frame = pool.begin_frame(first.clone());
        first.encodeWaitForEvent_value(event, 1);
        let first_pixels = copy_to_readback(&device, &first, &pool, &original);
        frame.submit();

        let second = queue.commandBuffer().unwrap();
        let frame = pool.begin_frame(second.clone());
        second.encodeWaitForEvent_value(event, 2);
        let second_pixels = copy_to_readback(&device, &second, &pool, &original);
        frame.submit();
        drop(original); // Simulate eviction while two encoded frames still read it.

        gate.0.setSignaledValue(1);
        first.waitUntilCompleted();
        assert_eq!(first.status(), MTLCommandBufferStatus::Completed);
        assert_ne!(second.status(), MTLCommandBufferStatus::Completed);
        let third = queue.commandBuffer().unwrap();
        let frame = pool.begin_frame(third.clone());
        let replacement = pool.upload(&device, &[0x77u8; 256]).unwrap();
        assert_ne!(address(&replacement), original_address,
            "finishing one reader must not recycle storage still used by another");
        let third_pixels = copy_to_readback(&device, &third, &pool, &replacement);
        frame.submit();
        gate.0.setSignaledValue(2);
        third.waitUntilCompleted();
        assert_eq!(third.status(), MTLCommandBufferStatus::Completed);
        assert_eq!(bytes(&first_pixels, 256), vec![0x11; 256]);
        assert_eq!(bytes(&second_pixels, 256), vec![0x11; 256]);
        assert_eq!(bytes(&third_pixels, 256), vec![0x77; 256]);

        let frame = pool.begin_frame(queue.commandBuffer().unwrap());
        let reused = pool.upload(&device, &[0x99u8; 256]).unwrap();
        assert_eq!(address(&reused), original_address);
        assert_eq!(pool.stats().allocations, 2);
        assert_eq!(pool.stats().reuses, 1);
        drop(frame);
    }
}
