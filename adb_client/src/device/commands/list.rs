use std::io::Write;

use crate::{
    device::adb_message_device::ADBMessageDevice, ADBListItem, ADBMessageTransport, Result,
};

impl<T: ADBMessageTransport> ADBMessageDevice<T> {
    /// List the entries in the given directory on the device.
    pub(crate) fn list<A: AsRef<str>>(&mut self, source: A) -> Result<Vec<ADBListItem>> {
        self.begin_synchronization()?;
        let source = source.as_ref();

        self.end_transaction()?;
        // Ok()
        todo!()
    }
}
