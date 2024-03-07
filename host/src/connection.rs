use bt_hci::{
    cmd::link_control::DisconnectParams,
    param::{ConnHandle, DisconnectReason},
};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{DynamicReceiver, DynamicSender},
};

use crate::{
    adapter::{Adapter, ControlCommand},
    pdu::Pdu,
};

#[derive(Clone)]
pub struct Connection<'d> {
    handle: ConnHandle,
    tx: DynamicSender<'d, (ConnHandle, Pdu<'d>)>,
    control: DynamicSender<'d, ControlCommand>,
    event: DynamicReceiver<'d, ConnEvent>,
}

// An event related to this connection
pub(crate) enum ConnEvent {
    Bound(ConnHandle, u16),
    Unbound(ConnHandle, u16),
}

impl<'d> Connection<'d> {
    pub async fn accept<M: RawMutex, const CHANNELS: usize, const L2CAP_TXQ: usize, const L2CAP_RXQ: usize>(
        adapter: &'d Adapter<'d, M, CHANNELS, L2CAP_TXQ, L2CAP_RXQ>,
    ) -> Self {
        let event = adapter.acceptor.receive().await;
        Connection {
            handle: event.handle,
            tx: adapter.outbound.sender().into(),
            control: adapter.control.sender().into(),
            event: event.events,
        }
    }

    pub async fn disconnect(&mut self) {
        self.control
            .send(ControlCommand::Disconnect(DisconnectParams {
                handle: self.handle,
                reason: DisconnectReason::RemoteUserTerminatedConn,
            }))
            .await;
    }
}
