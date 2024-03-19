use core::future::poll_fn;

use crate::advertise::AdvertiseConfig;
use crate::attribute::AttributeTable;
use crate::attribute_server::AttributeServer;
use crate::channel_manager::ChannelManager;
use crate::connection::Connection;
use crate::connection_manager::{ConnectionInfo, ConnectionManager};
use crate::cursor::{ReadCursor, WriteCursor};
use crate::gatt::GattServer;
use crate::l2cap::{L2capPacket, L2CAP_CID_ATT, L2CAP_CID_DYN_START, L2CAP_CID_LE_U_SIGNAL};
use crate::packet_pool::{self, DynamicPacketPool, PacketPool, Qos, ATT_ID};
use crate::pdu::Pdu;
use crate::scan::{ScanConfig, ScanReport};
use crate::types::l2cap::L2capLeSignal;
use crate::{codec, Error};
use bt_hci::cmd::controller_baseband::SetEventMask;
use bt_hci::cmd::le::{
    LeCreateConn, LeCreateConnParams, LeSetAdvData, LeSetAdvEnable, LeSetAdvParams, LeSetScanEnable, LeSetScanParams,
};
use bt_hci::cmd::link_control::{Disconnect, DisconnectParams};
use bt_hci::cmd::{AsyncCmd, Cmd, SyncCmd};
use bt_hci::data::{AclBroadcastFlag, AclPacket, AclPacketBoundary};
use bt_hci::event::le::LeEvent;
use bt_hci::event::Event;
use bt_hci::param::{BdAddr, ConnHandle, DisconnectReason, EventMask};
use bt_hci::{Driver, FromHciBytes, PacketKind, WriteHci};
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

pub struct Adapter<
    'd,
    M,
    T,
    const CONNS: usize,
    const CHANNELS: usize,
    const L2CAP_TXQ: usize = 1,
    const L2CAP_RXQ: usize = 1,
> where
    M: RawMutex,
{
    driver: RefCell<T>,
    pub(crate) connections: ConnectionManager<M, CONNS>,
    pub(crate) channels: ChannelManager<'d, M, CHANNELS, L2CAP_TXQ, L2CAP_RXQ>,
    pub(crate) att_inbound: Channel<M, (ConnHandle, Pdu<'d>), L2CAP_RXQ>,
    pub(crate) pool: &'d dyn DynamicPacketPool<'d>,

    pub(crate) outbound: Channel<M, (ConnHandle, Pdu<'d>), L2CAP_TXQ>,
    pub(crate) control: Channel<M, ControlCommand, 1>,
    pub(crate) scanner: Channel<M, ScanReport, 1>,
}

pub(crate) enum ControlCommand {
    Disconnect(DisconnectParams),
    Connect(LeCreateConnParams),
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

impl<'d, M, T, const CONNS: usize, const CHANNELS: usize, const L2CAP_TXQ: usize, const L2CAP_RXQ: usize>
    Adapter<'d, M, T, CONNS, CHANNELS, L2CAP_TXQ, L2CAP_RXQ>
where
    M: RawMutex,
    T: Driver,
{
    const NEW_L2CAP: Channel<M, Pdu<'d>, L2CAP_RXQ> = Channel::new();

    /// Create a new instance of the BLE host adapter.
    ///
    /// The adapter requires a HCI driver (a particular HCI-compatible controller implementing the required traits), and
    /// a reference to resources that are created outside the adapter but which the adapter is the only accessor of.
    pub fn new<const PACKETS: usize, const L2CAP_MTU: usize>(
        driver: T,
        host_resources: &'d mut HostResources<M, CHANNELS, PACKETS, L2CAP_MTU>,
    ) -> Self {
        Self {
            driver: RefCell::new(driver),
            connections: ConnectionManager::new(),
            channels: ChannelManager::new(&host_resources.pool),
            pool: &host_resources.pool,
            att_inbound: Channel::new(),
            scanner: Channel::new(),

            outbound: Channel::new(),
            control: Channel::new(),
        }
    }

    /// Performs a BLE scan, return a report for discovering peripherals.
    ///
    /// Scan is stopped when a report is received. Call this method repeatedly to continue scanning.
    pub async fn scan(&self, config: &ScanConfig) -> Result<ScanReport, Error<T::Error>> {
        //let mut tx = [0; 259];
        //let mut rx = [0; 259];
        //let params = config.params.unwrap_or(LeSetScanParams::new(
        //    bt_hci::param::LeScanKind::Passive,
        //    bt_hci::param::Duration::from_millis(1_000),
        //    bt_hci::param::Duration::from_millis(1_000),
        //    bt_hci::param::AddrKind::PUBLIC,
        //    bt_hci::param::ScanningFilterPolicy::BasicUnfiltered,
        //));
        //params.exec(&self.controller).await?;

        //LeSetScanEnable::new(true, true).exec(&self.controller).await?;

        //let report = self.scanner.receive().await;
        //LeSetScanEnable::new(false, false).exec(&self.controller).await?;
        //Ok(report)
        todo!()
    }

    /// Starts sending BLE advertisements according to the provided config.
    ///
    /// Advertisements are stopped when a connection is made against this host,
    /// in which case a handle for the connection is returned.
    pub async fn advertise<'m>(&'m self, config: &AdvertiseConfig<'_>) -> Result<Connection<'m>, Error<T::Error>> {
        //let params = &config.params.unwrap_or(LeSetAdvParams::new(
        //    bt_hci::param::Duration::from_millis(400),
        //    bt_hci::param::Duration::from_millis(400),
        //    bt_hci::param::AdvKind::AdvInd,
        //    bt_hci::param::AddrKind::PUBLIC,
        //    bt_hci::param::AddrKind::PUBLIC,
        //    BdAddr::default(),
        //    bt_hci::param::AdvChannelMap::ALL,
        //    bt_hci::param::AdvFilterPolicy::default(),
        //));

        //params.exec(&self.controller).await?;

        //let mut data = [0; 31];
        //let mut w = WriteCursor::new(&mut data[..]);
        //for item in config.data.iter() {
        //    item.encode(&mut w)?;
        //}
        //let len = w.len();
        //drop(w);
        //LeSetAdvData::new(len as u8, data).exec(&self.controller).await?;
        //LeSetAdvEnable::new(true).exec(&self.controller).await?;
        //let conn = Connection::accept(self).await;
        //LeSetAdvEnable::new(false).exec(&self.controller).await?;
        //Ok(conn)
        todo!()
    }

    /// Creates a GATT server capable of processing the GATT protocol using the provided table of attributes.
    pub fn gatt_server<'reference, 'values, const MAX: usize>(
        &'reference self,
        table: &'reference AttributeTable<'values, M, MAX>,
    ) -> GattServer<'reference, 'values, 'd, M, MAX> {
        GattServer {
            server: AttributeServer::new(table),
            pool: self.pool,
            pool_id: packet_pool::ATT_ID,
            rx: self.att_inbound.receiver().into(),
            tx: self.outbound.sender().into(),
            connections: &self.connections,
        }
    }

    async fn handle_acl(&self, acl: AclPacket<'_>) -> Result<(), HandleError> {
        let (conn, packet) = L2capPacket::decode(acl)?;
        match packet.channel {
            L2CAP_CID_ATT => {
                if let Some(mut p) = self.pool.alloc(ATT_ID) {
                    let len = packet.payload.len();
                    p.as_mut()[..len].copy_from_slice(packet.payload);
                    self.att_inbound
                        .send((
                            conn,
                            Pdu {
                                packet: p,
                                pb: acl.boundary_flag(),
                                len,
                            },
                        ))
                        .await;
                } else {
                    // TODO: Signal back
                }
            }
            L2CAP_CID_LE_U_SIGNAL => {
                let mut r = ReadCursor::new(packet.payload);
                let signal: L2capLeSignal = r.read()?;
                match self.channels.control(conn, signal).await {
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

    async fn write_command<C: Cmd>(&self, command: C, tx: &mut [u8]) -> Result<(), Error<T::Error>> {}

    pub async fn run(&self) -> Result<(), Error<T::Error>> {
        SetEventMask::new(
            EventMask::new()
                .enable_le_meta(true)
                .enable_conn_request(true)
                .enable_conn_complete(true)
                .enable_hardware_error(true)
                .enable_disconnection_complete(true),
        )
        .exec(&self.controller)
        .await?;

        loop {
            let mut rx = [0u8; 259];
            let mut tx = [0u8; 259];
            // info!("Entering select");
            match select4(
                poll_fn(|cx| {
                    let mut c = self.driver.borrow_mut();
                    match c.try_read(&mut rx) {
                        Ok(None) => {
                            c.register_read_waker(cx.waker());
                            Poll::Pending
                        }
                        Ok(Some(kind)) => Poll::Ready(Ok(kind)),
                        Err(e) => Poll::Ready(Err(e)),
                    }
                }),
                self.outbound.receive(),
                self.control.receive(),
                self.channels.signal(),
            )
            .await
            {
                Either4::First(kind) => {
                    // info!("Incoming event");
                    match kind {
                        Ok(PacketKind::AclData) => {
                            let acl = AclPacket::from_hci_bytes(&rx)?;
                            match self.handle_acl(acl).await {
                                Ok(_) => {}
                                Err(e) => {
                                    info!("Error processing ACL packet: {:?}", e);
                                }
                            }
                        }
                        Ok(PacketKind::Event) => {
                            let event = Event::from_hci_bytes(&rx)?;
                            match event {
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
                                                att_mtu: 23,
                                            },
                                        ) {
                                            warn!("Error establishing connection: {:?}", err);
                                            Disconnect::new(
                                                e.handle,
                                                DisconnectReason::RemoteDeviceTerminatedConnLowResources,
                                            )
                                            .exec(&self.controller)
                                            .await
                                            .unwrap();
                                        }
                                    }
                                    LeEvent::LeAdvertisingReport(data) => {
                                        self.scanner
                                            .send(ScanReport::new(data.reports.num_reports, &data.reports.bytes))
                                            .await;
                                    }
                                    _ => {
                                        warn!("Unknown event: {:?}", event);
                                    }
                                },
                                Event::DisconnectionComplete(e) => {
                                    info!("Disconnected: {:?}", e);
                                    let _ = self.connections.disconnect(e.handle);
                                }
                                Event::NumberOfCompletedPackets(c) => {
                                    //info!("Confirmed {} packets sent", c.completed_packets.len());
                                }
                                _ => {
                                    warn!("Unknown event: {:?}", event);
                                }
                            }
                        }
                        Ok(kind) => {
                            info!("Ignoring packet with kind: {:?}", p);
                        }
                        Err(e) => {
                            info!("Error from controller: {:?}", e);
                        }
                    }
                }
                Either4::Second((handle, pdu)) => {
                    // info!("Outgoing packet");
                    let acl = AclPacket::new(handle, pdu.pb, AclBroadcastFlag::PointToPoint, pdu.as_ref());
                    let len = acl.size();
                    acl.write_hci(&mut tx)?;
                    match poll_fn(|cx| {
                        let mut c = self.driver.borrow_mut();
                        match c.try_write(&tx[..len]) {
                            Ok(None) => {
                                c.register_write_waker(cx.waker());
                                Poll::Pending
                            }
                            Ok(Some(_)) => Poll::Ready(Ok(())),
                            Err(e) => Poll::Ready(Err(e)),
                        }
                    })
                    .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            warn!("Error writing some ACL data to controller: {:?}", e);
                            panic!(":(");
                        }
                    }
                }
                Either4::Third(command) => {
                    // info!("Outgoing command");
                    match command {
                        ControlCommand::Connect(params) => {
                            LeSetScanEnable::new(false, false).exec(&self.controller).await.unwrap();
                            LeCreateConn::new(
                                params.le_scan_interval,
                                params.le_scan_window,
                                params.use_filter_accept_list,
                                params.peer_addr_kind,
                                params.peer_addr,
                                params.own_addr_kind,
                                params.conn_interval_min,
                                params.conn_interval_max,
                                params.max_latency,
                                params.supervision_timeout,
                                params.min_ce_length,
                                params.max_ce_length,
                            )
                            .exec(&self.controller)
                            .await
                            .unwrap();
                        }
                        ControlCommand::Disconnect(params) => {
                            self.connections.disconnect(params.handle).unwrap();
                            Disconnect::new(params.handle, params.reason)
                                .exec(&self.controller)
                                .await
                                .unwrap();
                        }
                    }
                }
                Either4::Fourth((handle, response)) => {
                    // info!("Outgoing signal: {:?}", response);
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
                    match self.controller.write_acl_data(&acl).await {
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
