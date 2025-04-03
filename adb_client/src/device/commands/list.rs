use crate::{
    device::{
        adb_message_device::ADBMessageDevice, ADBTransportMessage, MessageCommand,
        MessageSubcommand,
    },
    ADBListItem, ADBListItemType, ADBMessageTransport, Result, RustADBError,
};
use byteorder::ByteOrder;
use byteorder::LittleEndian;
use std::str;

impl<T: ADBMessageTransport> ADBMessageDevice<T> {
    /// List the entries in the given directory on the device.
    pub(crate) fn list(&mut self, path: &str) -> Result<Vec<ADBListItem>> {
        self.begin_synchronization()?;

        let output = self.handle_list(path);

        self.end_transaction()?;
        output
    }

    fn handle_list(&mut self, path: &str) -> Result<Vec<ADBListItem>> {
        // TODO: See if recursive is possible
        // TODO: use LIS2 to support files over 2.14 GB in size.
        // SEE: https://github.com/cstyan/adbDocumentation?tab=readme-ov-file#adb-list
        // let mut len_buf = [0_u8; 4];
        let local_id = self.get_local_id()?;
        let remote_id = self.get_remote_id()?;
        {
            let mut len_buf = Vec::from([0_u8; 4]);
            LittleEndian::write_u32(&mut len_buf, path.len() as u32);

            let subcommand_data = MessageSubcommand::List; //.with_arg(path.len() as u32);

            let mut serialized_message =
                bincode::serialize(&subcommand_data).map_err(|_e| RustADBError::ConversionError)?;

            serialized_message.append(&mut len_buf);
            let mut path_bytes: Vec<u8> = Vec::from(path.as_bytes());
            serialized_message.append(&mut path_bytes);
            drop(path_bytes);

            let message = ADBTransportMessage::new(
                MessageCommand::Write,
                local_id,
                remote_id,
                &serialized_message,
            );
            self.send_and_expect_okay(message)?;
        }

        let mut list_items = Vec::new();

        let transport = self.get_transport_mut();
        let mut response = transport.read_message()?;
        let mut payload = response.payload();
        let mut current_index = 0;
        loop {
            // Get the next response if we ran out of payload. The payload always ends directly after a file name
            if payload.len() == current_index {
                let message =
                    ADBTransportMessage::new(MessageCommand::Okay, local_id, remote_id, &[]);
                transport.write_message(message)?;
                response = transport.read_message()?;
                payload = response.payload();
                current_index = 0;
            }
            // Loop though the response for all the entries
            const STATUS_CODE_LENGTH_IN_BYTES: usize = 4;
            match str::from_utf8(
                &payload[current_index..current_index + STATUS_CODE_LENGTH_IN_BYTES],
            )? {
                "DENT" => {
                    // Increase the current index after reading the command thing
                    current_index += STATUS_CODE_LENGTH_IN_BYTES;

                    // Read the file mode, size, mod time and name length in one go, since all their sizes are predictable
                    const U32_SIZE_IN_BYTES: usize = 4;
                    let file_mod =
                        payload[current_index..current_index + U32_SIZE_IN_BYTES].to_vec();
                    current_index += U32_SIZE_IN_BYTES;
                    let file_size =
                        payload[current_index..current_index + U32_SIZE_IN_BYTES].to_vec();
                    current_index += U32_SIZE_IN_BYTES;
                    let mod_time =
                        payload[current_index..current_index + U32_SIZE_IN_BYTES].to_vec();
                    current_index += U32_SIZE_IN_BYTES;
                    let name_len =
                        payload[current_index..current_index + U32_SIZE_IN_BYTES].to_vec();
                    current_index += U32_SIZE_IN_BYTES;

                    let mode = LittleEndian::read_u32(&file_mod);
                    let size = LittleEndian::read_u32(&file_size);
                    let time = LittleEndian::read_u32(&mod_time);
                    let name_len = LittleEndian::read_u32(&name_len);
                    // Read the file name, since it requires the length from the name_len
                    let name_buf =
                        payload[current_index..current_index + name_len as usize].to_vec();
                    current_index += name_len as usize;
                    let name = String::from_utf8(name_buf)?;

                    // First 9 bits are the file permissions
                    let file_permissions = mode & 0b111111111;
                    // Bits 14 to 16 are the file type
                    let file_type = (mode >> 13) & 0b111;
                    let item_type = match file_type {
                        0b010 => ADBListItemType::Directory,
                        0b100 => ADBListItemType::File,
                        0b101 => ADBListItemType::Symlink,
                        _ => return Err(RustADBError::UnknownFileMode(mode)),
                    };
                    let entry = ADBListItem {
                        item_type,
                        name,
                        time,
                        size,
                        permissions: file_permissions,
                    };
                    list_items.push(entry);
                }
                "DONE" => {
                    return Ok(list_items);
                }
                x => log::error!("Got an unknown response {}", x),
            }
        }
    }
}
