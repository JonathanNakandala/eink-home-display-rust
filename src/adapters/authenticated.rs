//! Who a request is from, once a certificate has proved it.
//!
//! Over HTTPS a display shows the certificate the authority gave it, and that names it. The server
//! puts that name on the request as this, and whatever serves the request reads it from there. It is
//! kept apart from the `?device=` a display also sends on every request, which anyone can write:
//! where there is a name proved by a certificate, the query is not even looked at.

use crate::domain::models::device_id::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedDevice(pub DeviceId);
