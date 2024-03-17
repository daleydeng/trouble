use core::fmt;

use crate::att::{self, Att, ATT_HANDLE_VALUE_NTF_OPTCODE};
use crate::attribute::CharacteristicHandle;
use crate::attribute_server::AttributeServer;
use crate::connection::Connection;
use crate::connection_manager::DynamicConnectionManager;
use crate::cursor::WriteCursor;
use crate::packet_pool::{AllocId, DynamicPacketPool};
use crate::pdu::Pdu;
use crate::types::uuid::Uuid;
use bt_hci::param::ConnHandle;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{DynamicReceiver, DynamicSender};
use heapless::Vec;

pub struct GattServer<'reference, 'values, 'resources, M: RawMutex, const MAX: usize> {
    pub(crate) server: AttributeServer<'reference, 'values, M, MAX>,
    pub(crate) rx: DynamicReceiver<'reference, (ConnHandle, Pdu<'resources>)>,
    pub(crate) tx: DynamicSender<'reference, (ConnHandle, Pdu<'resources>)>,
    pub(crate) pool_id: AllocId,
    pub(crate) pool: &'resources dyn DynamicPacketPool<'resources>,
    pub(crate) connections: &'reference dyn DynamicConnectionManager,
}

impl<'reference, 'values, 'resources, M: RawMutex, const MAX: usize>
    GattServer<'reference, 'values, 'resources, M, MAX>
{
    pub async fn next(&self) -> Result<GattEvent<'reference, 'values>, ()> {
        loop {
            let (handle, pdu) = self.rx.receive().await;
            match Att::decode(pdu.as_ref()) {
                Ok(att) => {
                    let Some(mut response) = self.pool.alloc(self.pool_id) else {
                        return Err(());
                    };
                    let mut w = WriteCursor::new(response.as_mut());
                    let (mut header, mut data) = w.split(4).map_err(|_| ())?;

                    match att {
                        Att::ExchangeMtu { mtu } => {
                            let mtu = self.connections.exchange_att_mtu(handle, mtu);
                            data.write(att::ATT_EXCHANGE_MTU_RESPONSE_OPCODE).map_err(|_| ())?;
                            data.write(mtu).map_err(|_| ())?;

                            header.write(data.len() as u16).map_err(|_| ())?;
                            header.write(4 as u16).map_err(|_| ())?;
                            let len = header.len() + data.len();
                            drop(header);
                            drop(data);
                            drop(w);
                            self.tx.send((handle, Pdu::new(response, len))).await;
                        }
                        _ => match self.server.process(handle, att, data.write_buf()) {
                            Ok(Some(written)) => {
                                let mtu = self.connections.get_att_mtu(handle);
                                data.commit(written).map_err(|_| ())?;
                                data.truncate(mtu as usize);
                                header.write(written as u16).map_err(|_| ())?;
                                header.write(4 as u16).map_err(|_| ())?;
                                let len = header.len() + data.len();
                                drop(header);
                                drop(data);
                                drop(w);
                                self.tx.send((handle, Pdu::new(response, len))).await;
                            }
                            Ok(None) => {
                                debug!("No response sent");
                            }
                            Err(e) => {
                                warn!("Error processing attribute: {:?}", e);
                            }
                        },
                    }
                }
                Err(e) => {
                    warn!("Error decoding attribute request: {:02x}", e);
                }
            }
        }
    }

    /// Write a value to a characteristic, and notify a connection with the new value of the characteristic.
    ///
    /// If the provided connection has not subscribed for this characteristic, it will not be notified.
    ///
    /// If the characteristic for the handle cannot be found, an error is returned.
    pub async fn notify(
        &self,
        handle: CharacteristicHandle,
        connection: &Connection<'_>,
        value: &[u8],
    ) -> Result<(), ()> {
        let conn = connection.handle();
        self.server.table.set(handle, value).map_err(|_| ())?;

        let cccd_handle = handle.cccd_handle.ok_or(())?;

        if !self.server.should_notify(conn, cccd_handle) {
            // No reason to fail?
            return Ok(());
        }

        let Some(mut packet) = self.pool.alloc(self.pool_id) else {
            return Err(());
        };
        let mut w = WriteCursor::new(packet.as_mut());
        let (mut header, mut data) = w.split(4).map_err(|_| ())?;
        data.write(ATT_HANDLE_VALUE_NTF_OPTCODE).map_err(|_| ())?;
        data.write(handle.handle).map_err(|_| ())?;
        data.append(value).map_err(|_| ())?;

        header.write(data.len() as u16).map_err(|_| ())?;
        header.write(4 as u16).map_err(|_| ())?;
        let total = header.len() + data.len();
        drop(header);
        drop(data);
        drop(w);
        self.tx.send((conn, Pdu::new(packet, total))).await;
        Ok(())
    }
}

#[derive(Clone)]
pub enum GattEvent<'reference, 'values> {
    Write {
        connection: Connection<'reference>,
        handle: CharacteristicHandle,
        value: &'values [u8],
    },
}

impl<'reference, 'values> fmt::Debug for GattEvent<'reference, 'values> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write {
                connection: _,
                handle: _,
                value: _,
            } => f.debug_struct("GattEvent::Write").finish(),
        }
    }
}

#[cfg(feature = "defmt")]
impl<'reference, 'values> defmt::Format for GattEvent<'reference, 'values> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{}", defmt::Debug2Format(self))
    }
}

pub struct GattClient<'reference, 'resources> {
    pub(crate) tx: DynamicSender<'reference, (ConnHandle, Pdu<'resources>)>,
    pub(crate) rx: DynamicReceiver<'reference, (ConnHandle, Pdu<'resources>)>,
    pub(crate) pool_id: AllocId,
    pub(crate) pool: &'resources dyn DynamicPacketPool<'resources>,
}

impl<'reference, 'resources> GattClient<'reference, 'resources> {
    /// Discover a schema of handles/attributes
    pub async fn service<const MAX: usize>(&mut self) -> Result<ServiceClient<MAX>, ()> {
        todo!()
    }

    async fn send(&self, data: Att<'_>) -> Result<(), ()> {
        todo!()
    }

    async fn receive(&self, data: Att<'_>) -> Result<(), ()> {
        todo!()
    }
}

pub struct ServiceClient<'reference, 'resources, const MAX: usize> {
    gatt: &'reference GattClient<'reference, 'resources>,
    characteristics: Vec<(Uuid, CharacteristicHandle), MAX>,
}

pub struct CharacteristicClient<'reference, 'resources> {
    gatt: &'reference GattClient<'reference, 'resources>,
    handle: CharacteristicHandle,
    uuid: Uuid,
}

impl<'reference, 'resources, const MAX: usize> ServiceClient<'reference, 'resources, MAX> {
    pub async fn characteristic(&mut self, uuid: Uuid) -> Result<CharacteristicClient<'reference, 'resources>, ()> {
        todo!()
    }
}

impl<'reference, 'resources> CharacteristicClient<'reference, 'resources> {
    pub async fn write(&mut self, data: &[u8]) -> Result<(), ()> {
        todo!()
    }

    pub async fn read(&mut self, handle: CharacteristicHandle, data: &mut [u8]) -> Result<(), ()> {
        todo!()
    }

    pub async fn subscribe(&mut self, handle: CharacteristicHandle) -> Result<(), ()> {
        todo!()
    }
}
