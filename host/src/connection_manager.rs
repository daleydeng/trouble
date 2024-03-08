use core::{
    cell::RefCell,
    future::poll_fn,
    task::{Context, Poll},
};

use bt_hci::param::{BdAddr, ConnHandle, LeConnRole, Status};
use embassy_sync::{
    blocking_mutex::{raw::RawMutex, Mutex},
    waitqueue::WakerRegistration,
};

struct State<const CONNS: usize> {
    connections: [ConnectionState; CONNS],
    waker: WakerRegistration,
}

pub struct ConnectionManager<M: RawMutex, const CONNS: usize> {
    state: Mutex<M, RefCell<State<CONNS>>>,
}

impl<M: RawMutex, const CONNS: usize> ConnectionManager<M, CONNS> {
    const DISCONNECTED: ConnectionState = ConnectionState::Disconnected;
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(State {
                connections: [Self::DISCONNECTED; CONNS],
                waker: WakerRegistration::new(),
            })),
        }
    }

    pub fn disconnect(&self, h: ConnHandle) -> Result<(), ()> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            for storage in state.connections.iter_mut() {
                match storage {
                    ConnectionState::Connecting(handle, _) if *handle == h => {
                        *storage = ConnectionState::Disconnected;
                    }
                    ConnectionState::Connected(handle, _) if *handle == h => {
                        *storage = ConnectionState::Disconnected;
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }

    pub fn connect(&self, handle: ConnHandle, info: ConnectionInfo) -> Result<(), ()> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            for storage in state.connections.iter_mut() {
                match storage {
                    ConnectionState::Disconnected => {
                        *storage = ConnectionState::Connecting(handle, info);
                        state.waker.wake();
                        return Ok(());
                    }
                    _ => {}
                }
            }
            Err(())
        })
    }

    pub fn poll_accept(&self, cx: &mut Context<'_>) -> Poll<ConnHandle> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            for storage in state.connections.iter_mut() {
                match storage {
                    ConnectionState::Connecting(handle, info) => {
                        let handle = handle.clone();
                        *storage = ConnectionState::Connected(handle.clone(), info.clone());
                        return Poll::Ready(handle);
                    }
                    _ => {}
                }
            }
            state.waker.register(cx.waker());
            Poll::Pending
        })
    }

    pub async fn accept(&self) -> ConnHandle {
        poll_fn(move |cx| self.poll_accept(cx)).await
    }
}

pub enum ConnectionState {
    Disconnected,
    Connecting(ConnHandle, ConnectionInfo),
    Connected(ConnHandle, ConnectionInfo),
}

#[derive(Clone, Copy)]
pub struct ConnectionInfo {
    pub handle: ConnHandle,
    pub status: Status,
    pub role: LeConnRole,
    pub peer_address: BdAddr,
    pub interval: u16,
    pub latency: u16,
    pub timeout: u16,
}
