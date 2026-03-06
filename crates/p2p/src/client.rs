use tracing::{debug, info};

use crate::error::P2PError;
use crate::types::*;

/// HTTP client for communicating with peers over Tor.
pub struct P2PClient;

impl P2PClient {
    fn secret_header(secret: &str) -> Vec<(&str, &str)> {
        vec![
            ("X-Company-Secret", secret),
            ("Content-Type", "application/json"),
        ]
    }

    /// Ping a peer to check if they're online.
    pub async fn ping(onion: &str, secret: &str) -> Result<PingResponse, P2PError> {
        let url = format!("http://{}/api/ping", onion);
        let headers = Self::secret_header(secret);

        debug!(peer = %onion, "Pinging peer");
        let response = artiqwest::get(&url, Some(headers), None).await?;

        if response.status() != http::StatusCode::OK {
            return Err(P2PError::NetworkError(format!(
                "Ping failed with status: {}",
                response.status()
            )));
        }

        let ping: PingResponse = serde_json::from_slice(response.body())?;
        Ok(ping)
    }

    /// Perform a full-state sync with a peer.
    pub async fn sync_with_peer(
        onion: &str,
        secret: &str,
        payload: SyncPayload,
        our_onion: &str,
        our_peer_id: Option<&str>,
    ) -> Result<SyncResponse, P2PError> {
        let url = format!("http://{}/api/sync", onion);
        let headers = Self::secret_header(secret);

        let request = SyncRequest {
            protocol_version: PROTOCOL_VERSION,
            sender_peer_id: our_peer_id.unwrap_or_default().to_string(),
            sender_onion: our_onion.to_string(),
            sender_time: chrono::Utc::now().to_rfc3339(),
            payload,
        };

        let body = serde_json::to_string(&request)?;

        info!(peer = %onion, "Sending sync request");
        let response = artiqwest::post(&url, &body, Some(headers), None).await?;

        if response.status() != http::StatusCode::OK {
            let body_text = String::from_utf8_lossy(response.body());
            return Err(P2PError::NetworkError(format!(
                "Sync failed with status {}: {}",
                response.status(),
                body_text
            )));
        }

        let sync_response: SyncResponse = serde_json::from_slice(response.body())?;
        Ok(sync_response)
    }

    /// Join a company by contacting the admin's onion service.
    pub async fn join_company(
        admin_onion: &str,
        secret: &str,
        our_onion: &str,
        our_name: &str,
    ) -> Result<CompanyInfo, P2PError> {
        let url = format!("http://{}/api/join", admin_onion);
        let headers = Self::secret_header(secret);

        let request = JoinRequest {
            onion_address: our_onion.to_string(),
            name: our_name.to_string(),
        };

        let body = serde_json::to_string(&request)?;

        info!(admin = %admin_onion, "Sending join request");
        let response = artiqwest::post(&url, &body, Some(headers), None).await?;

        if response.status() != http::StatusCode::OK {
            let body_text = String::from_utf8_lossy(response.body());
            return Err(P2PError::NetworkError(format!(
                "Join failed with status {}: {}",
                response.status(),
                body_text
            )));
        }

        let info: CompanyInfo = serde_json::from_slice(response.body())?;
        Ok(info)
    }

    /// Announce an address change to a peer.
    pub async fn announce_address_change(
        peer_onion: &str,
        secret: &str,
        peer_id: &str,
        new_onion: &str,
    ) -> Result<(), P2PError> {
        let url = format!("http://{}/api/address-update", peer_onion);
        let headers = Self::secret_header(secret);

        let request = AddressUpdate {
            peer_id: peer_id.to_string(),
            new_onion_address: new_onion.to_string(),
        };

        let body = serde_json::to_string(&request)?;
        let response = artiqwest::post(&url, &body, Some(headers), None).await?;

        if response.status() != http::StatusCode::OK {
            return Err(P2PError::NetworkError(format!(
                "Address update failed with status: {}",
                response.status()
            )));
        }

        Ok(())
    }
}
