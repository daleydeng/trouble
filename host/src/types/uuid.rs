use crate::codec::{Decode, Encode, Error, Type};

#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Uuid {
    Uuid16([u8; 2]),
    Uuid128([u8; 16]),
}

impl Uuid {
    pub const fn new_short(val: u16) -> Self {
        Self::Uuid16(val.to_le_bytes())
    }

    pub const fn new_long(val: [u8; 16]) -> Self {
        Self::Uuid128(val)
    }

    pub fn bytes(&self, data: &mut [u8]) {
        match self {
            Uuid::Uuid16(uuid) => data.copy_from_slice(uuid),
            Uuid::Uuid128(uuid) => data.copy_from_slice(uuid),
        }
    }

    pub fn get_type(&self) -> u8 {
        match self {
            Uuid::Uuid16(_) => 0x01,
            Uuid::Uuid128(_) => 0x02,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Uuid::Uuid16(_) => 6,
            Uuid::Uuid128(_) => 20,
        }
    }

    pub fn as_raw(&self) -> &[u8] {
        match self {
            Uuid::Uuid16(uuid) => uuid,
            Uuid::Uuid128(uuid) => uuid,
        }
    }
}

impl From<u16> for Uuid {
    fn from(data: u16) -> Self {
        Uuid::Uuid16(data.to_le_bytes())
    }
}

impl From<&[u8]> for Uuid {
    fn from(data: &[u8]) -> Self {
        match data.len() {
            2 => Uuid::Uuid16(data.try_into().unwrap()),
            16 => {
                let bytes: [u8; 16] = data.try_into().unwrap();
                Uuid::Uuid128(bytes)
            }
            _ => panic!(),
        }
    }
}

impl Type for Uuid {
    fn size(&self) -> usize {
        self.as_raw().len()
    }
}

impl Decode for Uuid {
    fn decode(src: &[u8]) -> Result<Self, Error> {
        if src.len() < 2 {
            Err(Error::InvalidValue)
        } else {
            let val: u16 = u16::from_le_bytes([src[0], src[1]]);
            // Must be a long id
            if val == 0 {
                if src.len() < 16 {
                    return Err(Error::InvalidValue);
                }
                Ok(Uuid::Uuid128(src[0..16].try_into().map_err(|_| Error::InvalidValue)?))
            } else {
                Ok(Uuid::new_short(val))
            }
        }
    }
}

impl Encode for Uuid {
    fn encode(&self, dest: &mut [u8]) -> Result<(), Error> {
        self.bytes(dest);
        Ok(())
    }
}
