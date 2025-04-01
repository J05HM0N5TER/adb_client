use crate::{
    models::{ADBListItem, ADBListItemType, AdbServerCommand, SyncCommand},
    ADBServerDevice, Result, RustADBError,
};
use byteorder::{ByteOrder, LittleEndian};
use std::{
    io::{Read, Write},
    str,
};

impl ADBServerDevice {
    /// Lists files in path on the device.
    /// note: path uses internal file paths, so Documents is at /storage/emulated/0/Documents
    pub fn list<A: AsRef<str>>(&mut self, path: A) -> Result<Vec<ADBListItem>> {
        self.set_serial_transport()?;

        // Set device in SYNC mode
        self.transport.send_adb_request(AdbServerCommand::Sync)?;

        // Send a list command
        self.transport.send_sync_request(SyncCommand::List)?;

        self.handle_list_command(path)
    }

    fn handle_list_command<S: AsRef<str>>(&mut self, path: S) -> Result<Vec<ADBListItem>> {
        // TODO: See if recursive is possible
        // TODO: use LIS2 to support files over 2.14 GB in size.
        // SEE: https://github.com/cstyan/adbDocumentation?tab=readme-ov-file#adb-list
        let mut len_buf = [0_u8; 4];
        LittleEndian::write_u32(&mut len_buf, path.as_ref().len() as u32);

        // 4 bytes of command name is already sent by send_sync_request
        self.transport.get_raw_connection()?.write_all(&len_buf)?;

        // List send the string of the directory to list, and then the server send a list of files
        self.transport
            .get_raw_connection()?
            .write_all(path.as_ref().to_string().as_bytes())?;

        let mut list_items = Vec::new();

        // Reads returned status code from ADB server
        let mut response = [0_u8; 4];
        loop {
            self.transport
                .get_raw_connection()?
                .read_exact(&mut response)?;
            match str::from_utf8(response.as_ref())? {
                "DENT" => {
                    let mut file_mod = [0_u8; 4];
                    let mut file_size = [0_u8; 4];
                    let mut mod_time = [0_u8; 4];
                    let mut name_len = [0_u8; 4];

                    let mut connection = self.transport.get_raw_connection()?;
                    connection.read_exact(&mut file_mod)?;
                    connection.read_exact(&mut file_size)?;
                    connection.read_exact(&mut mod_time)?;
                    connection.read_exact(&mut name_len)?;

                    let mode = LittleEndian::read_u32(&file_mod);
                    let size = LittleEndian::read_u32(&file_size);
                    let time = LittleEndian::read_u32(&mod_time);
                    let name_len = LittleEndian::read_u32(&name_len);
                    let mut name_buf = vec![0_u8; name_len as usize];
                    connection.read_exact(&mut name_buf)?;
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
