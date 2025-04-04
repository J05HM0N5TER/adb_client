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

    /// Request amount of bytes from transport, potentially across payloads
    ///
    /// This automatically request a new payload by sending back "Okay" and waiting for the next payload
    /// It reads the request bytes across the existing payload, and if there is not enough bytes left,
    /// reads the rest from the next payload
    ///
    ///   Current index                  
    /// ┼───────────────┼   Requested    
    ///                 ┌─────────────┐  
    /// ┌───────────────┼───────┐     │  
    /// └───────────────────────┘        
    ///     Current             └─────┘  
    ///     payload          Wanted in   
    ///                      Next payload
    fn read_bytes_from_transport(
        requested_bytes: &usize,
        current_index: &mut usize,
        transport: &mut T,
        payload: &mut Vec<u8>,
        local_id: &u32,
        remote_id: &u32,
    ) -> Result<Vec<u8>> {
        if *current_index + requested_bytes <= payload.len() {
            // if there is enough bytes in this payload
            // Copy from existing payload
            let slice = &payload[*current_index..*current_index + requested_bytes];
            *current_index += requested_bytes;
            Ok(slice.to_vec())
        } else {
            // Read the rest of the existing payload, then continue with the next message
            let mut slice = Vec::new();
            let read_from_existing_payload = payload.len() - *current_index;
            slice.extend_from_slice(
                &payload[*current_index..*current_index + read_from_existing_payload],
            );

            // Request the next message
            let send_message =
                ADBTransportMessage::new(MessageCommand::Okay, *local_id, *remote_id, &[]);
            transport.write_message(send_message)?;
            // Read the new message
            *payload = transport.read_message()?.into_payload();
            let bytes_read_from_new_payload = requested_bytes - read_from_existing_payload;
            slice.extend_from_slice(&payload[..bytes_read_from_new_payload]);
            *current_index = bytes_read_from_new_payload;
            Ok(slice)
        }
    }

    fn handle_list(&mut self, path: &str) -> Result<Vec<ADBListItem>> {
        // TODO: See if recursive is possible
        // TODO: use LIS2 to support files over 2.14 GB in size.
        // SEE: https://github.com/cstyan/adbDocumentation?tab=readme-ov-file#adb-list
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
        let mut payload = transport.read_message()?.into_payload();
        let mut current_index = 0;
        loop {
            // Loop though the response for all the entries
            const STATUS_CODE_LENGTH_IN_BYTES: usize = 4;
            let status_code = Self::read_bytes_from_transport(
                &STATUS_CODE_LENGTH_IN_BYTES,
                &mut current_index,
                transport,
                &mut payload,
                &local_id,
                &remote_id,
            )?;
            match str::from_utf8(&status_code)? {
                "DENT" => {
                    // Read the file mode, size, mod time and name length in one go, since all their sizes are predictable
                    const U32_SIZE_IN_BYTES: usize = 4;
                    let mode = Self::read_bytes_from_transport(
                        &U32_SIZE_IN_BYTES,
                        &mut current_index,
                        transport,
                        &mut payload,
                        &local_id,
                        &remote_id,
                    )?;
                    let size = Self::read_bytes_from_transport(
                        &U32_SIZE_IN_BYTES,
                        &mut current_index,
                        transport,
                        &mut payload,
                        &local_id,
                        &remote_id,
                    )?;
                    let time = Self::read_bytes_from_transport(
                        &U32_SIZE_IN_BYTES,
                        &mut current_index,
                        transport,
                        &mut payload,
                        &local_id,
                        &remote_id,
                    )?;
                    let name_len = Self::read_bytes_from_transport(
                        &U32_SIZE_IN_BYTES,
                        &mut current_index,
                        transport,
                        &mut payload,
                        &local_id,
                        &remote_id,
                    )?;

                    let mode = LittleEndian::read_u32(&mode);
                    let size = LittleEndian::read_u32(&size);
                    let time = LittleEndian::read_u32(&time);
                    let name_len = LittleEndian::read_u32(&name_len) as usize;
                    // Read the file name, since it requires the length from the name_len
                    let name_buf = Self::read_bytes_from_transport(
                        &name_len,
                        &mut current_index,
                        transport,
                        &mut payload,
                        &local_id,
                        &remote_id,
                    )?;
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
