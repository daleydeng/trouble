use core::cell::{RefCell, UnsafeCell};

use embassy_sync::blocking_mutex::{raw::RawMutex, Mutex};

// Generic client ID used by ATT PDU
pub(crate) const ATT_ID: AllocId = AllocId(0);
pub(crate) const L2CAP_SIGNAL_ID: AllocId = AllocId(1);

#[derive(Clone, Copy)]
pub struct AllocId(usize);

impl AllocId {
    pub fn dynamic(idx: usize) -> AllocId {
        // Dynamic range starts at 2
        AllocId(2 + idx)
    }
}

struct PacketBuf<const MTU: usize> {
    buf: [u8; MTU],
    free: bool,
}

impl<const MTU: usize> PacketBuf<MTU> {
    const NEW: PacketBuf<MTU> = PacketBuf::new();

    pub const fn new() -> Self {
        Self {
            buf: [0; MTU],
            free: true,
        }
    }
}

/// Quality of service policy for packet allocation
#[derive(Clone, Copy)]
pub enum Qos {
    /// Distribute evenly among client
    Fair,
    /// Reserve at least N packets for each client
    Guaranteed(usize),
    /// No guarantees
    None,
}

struct State<const MTU: usize, const N: usize, const CLIENTS: usize> {
    packets: UnsafeCell<[PacketBuf<MTU>; N]>,
    usage: RefCell<[usize; CLIENTS]>,
}

impl<const MTU: usize, const N: usize, const CLIENTS: usize> State<MTU, N, CLIENTS> {
    pub const fn new() -> Self {
        Self {
            packets: UnsafeCell::new([PacketBuf::NEW; N]),
            usage: RefCell::new([0; CLIENTS]),
        }
    }

    fn available(&self, qos: Qos, client: AllocId) -> usize {
        let usage = self.usage.borrow();
        match qos {
            Qos::None => N.checked_sub(usage.iter().sum()).unwrap_or(0),
            Qos::Fair => (N / CLIENTS).checked_sub(usage[client.0]).unwrap_or(0),
            Qos::Guaranteed(n) => {
                // Reserved for clients that should have minimum
                let reserved = n * usage.iter().filter(|c| **c == 0).count();
                let reserved = reserved - if usage[client.0] < n { n - usage[client.0] } else { 0 };
                let usage = reserved + usage.iter().sum::<usize>();
                N.checked_sub(usage).unwrap_or(0)
            }
        }
    }

    fn alloc(&self, id: AllocId) -> Option<PacketRef> {
        let mut usage = self.usage.borrow_mut();
        let packets = unsafe { &mut *self.packets.get() };
        for (idx, packet) in packets.iter_mut().enumerate() {
            if packet.free {
                packet.free = true;
                usage[id.0] += 1;
                return Some(PacketRef {
                    idx,
                    buf: &mut packet.buf[..],
                });
            }
        }
        None
    }

    fn free(&self, id: AllocId, p_ref: PacketRef) {
        let mut usage = self.usage.borrow_mut();
        let packets = unsafe { &mut *self.packets.get() };
        packets[p_ref.idx].free = true;
        usage[id.0] -= 1;
    }
}

/// A packet pool holds a pool of packet buffers that can be dynamically allocated
/// and free'd.
///
/// The pool has a concept QoS where it
pub struct PacketPool<M: RawMutex, const MTU: usize, const N: usize, const CLIENTS: usize> {
    state: Mutex<M, State<MTU, N, CLIENTS>>,
    qos: Qos,
}

impl<M: RawMutex, const MTU: usize, const N: usize, const CLIENTS: usize> PacketPool<M, MTU, N, CLIENTS> {
    pub const fn new(qos: Qos) -> Self {
        Self {
            state: Mutex::new(State::new()),
            qos,
        }
    }

    fn alloc(&self, id: AllocId) -> Option<Packet> {
        self.state.lock(|state| {
            let available = state.available(self.qos, id);
            if available == 0 {
                return None;
            }

            return state.alloc(id).map(|p_ref| Packet {
                client: id,
                p_ref: Some(p_ref),
                pool: self,
            });
        })
    }

    fn free(&self, id: AllocId, p_ref: PacketRef) {
        self.state.lock(|state| {
            state.free(id, p_ref);
        });
    }

    fn available(&self, id: AllocId) -> usize {
        self.state.lock(|state| state.available(self.qos, id))
    }
}

pub trait DynamicPacketPool<'d> {
    fn alloc(&'d self, id: AllocId) -> Option<Packet<'d>>;
    fn free(&self, id: AllocId, r: PacketRef);
    fn available(&self, id: AllocId) -> usize;
}

impl<'d, M: RawMutex, const MTU: usize, const N: usize, const CLIENTS: usize> DynamicPacketPool<'d>
    for PacketPool<M, MTU, N, CLIENTS>
{
    fn alloc(&'d self, id: AllocId) -> Option<Packet<'d>> {
        PacketPool::alloc(self, id)
    }

    fn available(&self, id: AllocId) -> usize {
        PacketPool::available(self, id)
    }

    fn free(&self, id: AllocId, r: PacketRef) {
        PacketPool::free(self, id, r)
    }
}

pub struct PacketRef {
    idx: usize,
    buf: *mut [u8],
}

pub struct Packet<'d> {
    client: AllocId,
    p_ref: Option<PacketRef>,
    pool: &'d dyn DynamicPacketPool<'d>,
}

impl<'d> Drop for Packet<'d> {
    fn drop(&mut self) {
        if let Some(r) = self.p_ref.take() {
            self.pool.free(self.client, r);
        }
    }
}

impl<'d> AsRef<[u8]> for Packet<'d> {
    fn as_ref(&self) -> &[u8] {
        let p = self.p_ref.as_ref().unwrap();
        unsafe { &(*p.buf)[..] }
    }
}

impl<'d> AsMut<[u8]> for Packet<'d> {
    fn as_mut(&mut self) -> &mut [u8] {
        let p = self.p_ref.as_mut().unwrap();
        unsafe { &mut (*p.buf)[..] }
    }
}

#[cfg(test)]
mod tests {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::*;

    #[test]
    fn test_fair_qos() {
        let pool: PacketPool<NoopRawMutex, 1, 8, 4> = PacketPool::new(Qos::Fair);

        let a1 = pool.alloc(0);
        assert!(a1.is_some());
        let a2 = pool.alloc(0);
        assert!(a2.is_some());
        assert!(pool.alloc(0).is_none());
        drop(a2);
        let a3 = pool.alloc(0);
        assert!(a3.is_some());

        let b1 = pool.alloc(1);
        assert!(b1.is_some());

        let c1 = pool.alloc(2);
        assert!(c1.is_some());
    }

    #[test]
    fn test_none_qos() {
        let pool: PacketPool<NoopRawMutex, 1, 8, 4> = PacketPool::new(Qos::None);

        let a1 = pool.alloc(0);
        assert!(a1.is_some());
        let a2 = pool.alloc(0);
        assert!(a2.is_some());
        let a3 = pool.alloc(0);
        assert!(a3.is_some());
        let a4 = pool.alloc(0);
        assert!(a4.is_some());
        let a5 = pool.alloc(0);
        assert!(a5.is_some());
        let a6 = pool.alloc(0);
        assert!(a6.is_some());
        let a7 = pool.alloc(0);
        assert!(a7.is_some());

        let b1 = pool.alloc(1);
        assert!(b1.is_some());

        let b2 = pool.alloc(1);
        assert!(b2.is_none());
    }

    #[test]
    fn test_guaranteed_qos() {
        let pool: PacketPool<NoopRawMutex, 1, 8, 4> = PacketPool::new(Qos::Guaranteed(1));

        let a1 = pool.alloc(0);
        assert!(a1.is_some());
        let a2 = pool.alloc(0);
        assert!(a2.is_some());
        let a3 = pool.alloc(0);
        assert!(a3.is_some());
        let a4 = pool.alloc(0);
        assert!(a4.is_some());
        let a5 = pool.alloc(0);
        assert!(a5.is_some());
        // Needs at least 3 for the other clients
        assert!(pool.alloc(0).is_none());

        let b1 = pool.alloc(1);
        assert!(b1.is_some());
        assert!(pool.alloc(1).is_none());

        let c1 = pool.alloc(2);
        assert!(c1.is_some());
        assert!(pool.alloc(2).is_none());

        let d1 = pool.alloc(3);
        assert!(d1.is_some());
        assert!(pool.alloc(3).is_none());
    }
}
