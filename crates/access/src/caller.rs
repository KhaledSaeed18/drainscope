//! Who is calling: the Unix user behind a bus connection.

use zbus::names::BusName;

/// The UID of the process owning the unique bus name `caller`, asked from the bus. `None` for
/// peer-to-peer connections (tests), which have no bus to ask.
///
/// # Errors
/// If the bus can't tell (e.g. the caller disconnected).
pub async fn caller_uid(
    connection: &zbus::Connection,
    caller: Option<&str>,
) -> zbus::Result<Option<u32>> {
    let Some(caller) = caller else {
        return Ok(None);
    };
    let name = BusName::try_from(caller)?;
    let uid = zbus::fdo::DBusProxy::new(connection)
        .await?
        .get_connection_unix_user(name)
        .await?;
    Ok(Some(uid))
}
