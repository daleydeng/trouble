use crate::ad_structure::AdStructure;
use crate::channel_manager::ChannelManager;
use crate::connection_manager::{ConnectionInfo, ConnectionManager};
use crate::cursor::{ReadCursor, WriteCursor};
use crate::l2cap::{L2capPacket, L2CAP_CID_ATT, L2CAP_CID_DYN_START, L2CAP_CID_LE_U_SIGNAL}; //self, L2capLeSignal, L2capPacket, L2capState, LeCreditConnReq, SignalCode};
use crate::packet_pool::{DynamicPacketPool, PacketPool, Qos, ATT_ID};
use crate::pdu::Pdu;
use crate::types::l2cap::L2capLeSignal;
use crate::{codec, Error};
use bt_hci::cmd::controller_baseband::SetEventMask;
use bt_hci::cmd::le::{LeSetAdvData, LeSetAdvEnable, LeSetAdvParams};
use bt_hci::cmd::link_control::{Disconnect, DisconnectParams};
use bt_hci::cmd::SyncCmd;
use bt_hci::data::{AclBroadcastFlag, AclPacket, AclPacketBoundary};
use bt_hci::event::le::LeEvent;
use bt_hci::event::Event;
use bt_hci::param::{BdAddr, ConnHandle, DisconnectReason, EventMask};
use bt_hci::ControllerCmdSync;
use bt_hci::ControllerToHostPacket;
use embassy_futures::select::{select4, Either4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::Channel;

pub struct HostResources<M: RawMutex, const CHANNELS: usize, const PACKETS: usize, const L2CAP_MTU: usize> {
    pool: PacketPool<M, L2CAP_MTU, PACKETS, CHANNELS>,
}

impl<M: RawMutex, const CHANNELS: usize, const PACKETS: usize, const L2CAP_MTU: usize>
    HostResources<M, CHANNELS, PACKETS, L2CAP_MTU>
{
    pub fn new(qos: Qos) -> Self {
        Self {
            pool: PacketPool::new(qos),
        }
    }
}

pub struct AdvertiseConfig<'d> {
    pub params: Option<LeSetAdvParams>,
    pub data: &'d [AdStructure<'d>],
}

pub struct Config<'a> {
    pub advertise: Option<AdvertiseConfig<'a>>,
}

impl<'a> Default for Config<'a> {
    fn default() -> Self {
        Self { advertise: None }
    }
}

pub struct Adapter<'d, M, const CONNS: usize, const CHANNELS: usize, const L2CAP_TXQ: usize, const L2CAP_RXQ: usize>
where
    M: RawMutex + 'd,
{
    pub(crate) connections: ConnectionManager<M, CONNS>,
    pub(crate) channels: ChannelManager<'d, M, CHANNELS, L2CAP_TXQ, L2CAP_RXQ>,
    pub(crate) att_inbound: Channel<M, (ConnHandle, Pdu<'d>), L2CAP_RXQ>,
    pub(crate) pool: &'d dyn DynamicPacketPool<'d>,

    pub(crate) outbound: Channel<M, (ConnHandle, Pdu<'d>), L2CAP_TXQ>,
    pub(crate) control: Channel<M, ControlCommand, 1>,
}

pub(crate) enum ControlCommand {
    Disconnect(DisconnectParams),
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum HandleError {
    Codec(codec::Error),
    Other,
}

impl From<codec::Error> for HandleError {
    fn from(e: codec::Error) -> Self {
        Self::Codec(e)
    }
}

impl<'d, M, const CONNS: usize, const CHANNELS: usize, const L2CAP_TXQ: usize, const L2CAP_RXQ: usize>
    Adapter<'d, M, CONNS, CHANNELS, L2CAP_TXQ, L2CAP_RXQ>
where
    M: RawMutex + 'd,
{
    const NEW_L2CAP: Channel<M, Pdu<'d>, L2CAP_RXQ> = Channel::new();
    pub fn new<const PACKETS: usize, const L2CAP_MTU: usize>(
        host_resources: &'d mut HostResources<M, CHANNELS, PACKETS, L2CAP_MTU>,
    ) -> Self {
        Self {
            connections: ConnectionManager::new(),
            channels: ChannelManager::new(&host_resources.pool),
            pool: &host_resources.pool,
            att_inbound: Channel::new(),

            outbound: Channel::new(),
            control: Channel::new(),
        }
    }

    pub async fn advertise<T>(&self, controller: &T, config: Config<'_>) -> Result<(), Error<T::Error>>
    where
        T: ControllerCmdSync<LeSetAdvData> + ControllerCmdSync<LeSetAdvEnable> + ControllerCmdSync<LeSetAdvParams>,
    {
        if let Some(adv) = &config.advertise {
            let params = &adv.params.unwrap_or(LeSetAdvParams::new(
                bt_hci::param::Duration::from_millis(400),
                bt_hci::param::Duration::from_millis(400),
                bt_hci::param::AdvKind::AdvInd,
                bt_hci::param::AddrKind::PUBLIC,
                bt_hci::param::AddrKind::PUBLIC,
                BdAddr::default(),
                bt_hci::param::AdvChannelMap::ALL,
                bt_hci::param::AdvFilterPolicy::default(),
            ));

            params.exec(controller).await?;

            let mut data = [0; 31];
            let mut w = WriteCursor::new(&mut data[..]);
            for item in adv.data.iter() {
                item.encode(&mut w)?;
            }
            let len = w.len();
            drop(w);
            LeSetAdvData::new(len as u8, data).exec(controller).await?;
            LeSetAdvEnable::new(true).exec(controller).await?;
        }
        Ok(())
    }

    async fn handle_acl(&self, acl: AclPacket<'_>) -> Result<(), HandleError> {
        let (conn, packet) = L2capPacket::decode(acl)?;
        match packet.channel {
            L2CAP_CID_ATT => {
                if let Some(mut p) = self.pool.alloc(ATT_ID) {
                    let len = packet.payload.len();
                    p.as_mut()[..len].copy_from_slice(packet.payload);
                    self.att_inbound.send((conn, Pdu { packet: p, len })).await;
                } else {
                    // TODO: Signal back
                }
            }
            L2CAP_CID_LE_U_SIGNAL => {
                let mut r = ReadCursor::new(packet.payload);
                let signal: L2capLeSignal = r.read()?;
                match self.channels.control(conn, signal) {
                    Ok(_) => {}
                    Err(_) => {
                        return Err(HandleError::Other);
                    }
                }
            }

            other if other >= L2CAP_CID_DYN_START => match self.channels.dispatch(packet).await {
                Ok(_) => {}
                Err(e) => {
                    warn!("Error dispatching l2cap packet to channel: {:?}", e);
                }
            },
            _ => {
                unimplemented!()
            }
        }
        Ok(())
    }

    pub async fn run<T>(&'d self, controller: &T) -> Result<(), Error<T::Error>>
    where
        T: ControllerCmdSync<Disconnect> + ControllerCmdSync<SetEventMask>,
    {
        SetEventMask::new(
            EventMask::new()
                .enable_le_meta(true)
                .enable_conn_request(true)
                .enable_conn_complete(true)
                .enable_hardware_error(true)
                .enable_disconnection_complete(true),
        )
        .exec(controller)
        .await?;

        loop {
            let mut rx = [0u8; 259];
            let mut tx = [0u8; 259];
            match select4(
                controller.read(&mut rx),
                self.outbound.receive(),
                self.control.receive(),
                self.channels.signal(),
            )
            .await
            {
                Either4::First(result) => match result {
                    Ok(ControllerToHostPacket::Acl(acl)) => match self.handle_acl(acl).await {
                        Ok(_) => {}
                        Err(e) => {
                            info!("Error processing ACL packet: {:?}", e);
                        }
                    },
                    Ok(ControllerToHostPacket::Event(event)) => match event {
                        Event::Le(event) => match event {
                            LeEvent::LeConnectionComplete(e) => {
                                if let Err(err) = self.connections.connect(
                                    e.handle,
                                    ConnectionInfo {
                                        handle: e.handle,
                                        status: e.status,
                                        role: e.role,
                                        peer_address: e.peer_addr,
                                        interval: e.conn_interval.as_u16(),
                                        latency: e.peripheral_latency,
                                        timeout: e.supervision_timeout.as_u16(),
                                    },
                                ) {
                                    warn!("Error establishing connection: {:?}", err);
                                    Disconnect::new(e.handle, DisconnectReason::RemoteDeviceTerminatedConnLowResources)
                                        .exec(controller)
                                        .await
                                        .unwrap();
                                }
                            }
                            _ => {
                                warn!("Unknown event: {:?}", event);
                            }
                        },
                        Event::DisconnectionComplete(e) => {
                            info!("Disconnected: {:?}", e);
                            let _ = self.connections.disconnect(e.handle);
                        }
                        Event::NumberOfCompletedPackets(c) => {}
                        _ => {
                            warn!("Unknown event: {:?}", event);
                        }
                    },
                    Ok(p) => {
                        info!("Ignoring packet: {:?}", p);
                    }
                    Err(e) => {
                        info!("Error from controller: {:?}", e);
                    }
                },
                Either4::Second((handle, pdu)) => {
                    let acl = AclPacket::new(
                        handle,
                        AclPacketBoundary::FirstNonFlushable,
                        AclBroadcastFlag::PointToPoint,
                        pdu.as_ref(),
                    );
                    info!("Sent packet to host");
                    match controller.write_acl_data(&acl).await {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Error writing some ACL data to controller: {:?}", e);
                            panic!(":(");
                        }
                    }
                }
                Either4::Third(command) => match command {
                    ControlCommand::Disconnect(params) => {
                        Disconnect::new(params.handle, params.reason)
                            .exec(controller)
                            .await
                            .unwrap();
                    }
                },
                Either4::Fourth((handle, response)) => {
                    let mut w = WriteCursor::new(&mut tx);
                    let (mut header, mut body) = w.split(4)?;

                    body.write(response)?;

                    // TODO: Move into l2cap packet type
                    header.write(body.len() as u16)?;
                    header.write(L2CAP_CID_LE_U_SIGNAL)?;
                    let len = header.len() + body.len();

                    header.finish();
                    body.finish();
                    w.finish();

                    let acl = AclPacket::new(
                        handle,
                        AclPacketBoundary::FirstNonFlushable,
                        AclBroadcastFlag::PointToPoint,
                        &tx[..len],
                    );
                    match controller.write_acl_data(&acl).await {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Error writing some ACL data to controller: {:?}", e);
                            panic!(":(");
                        }
                    }
                }
            }
        }
    }
}
